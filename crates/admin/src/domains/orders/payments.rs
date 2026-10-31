//! The payments of an order beyond its checkout: payment terms and their schedules, manual
//! payments, payments through a vaulted payment method (a mandate), payment reminders, and the
//! tender transactions every movement of money makes.
//!
//! As shopify.dev's reference of 2026-10 describes them (`mutations/<name>`, `objects/
//! PaymentTerms`, `objects/PaymentSchedule`, `queries/paymentTermsTemplates`,
//! `queries/tenderTransactions`):
//!
//! - **Templates**: the nine of `paymentTermsTemplates`' example, with its ids and names (Due on
//!   receipt, Due on fulfillment, Net 7 to Net 90, Fixed), seeded as `PaymentTermsTemplate`
//!   records.
//! - **Payment terms** (`paymentTermsCreate` on an order, a draft order's `paymentTerms` input,
//!   `paymentTermsUpdate`, `paymentTermsDelete`) have one schedule: net terms are due `dueInDays`
//!   after they are issued (the `issuedAt` given, else now for an order, else when the draft's
//!   invoice is sent or the draft completed); fixed terms on the `dueAt` given; terms due on
//!   receipt when issued; terms due on fulfillment once the order is fulfilled. A schedule's
//!   `balanceDue` is the order's outstanding amount, `totalBalance` what is still to be paid or
//!   authorized, `completedAt` when nothing is left. `due` once its due date passed by the
//!   shop's clock, `overdue` from the day after. Events: `payment_terms/create`, `…/update`,
//!   `…/delete`, with `orders/updated` (or `draft_orders/update`).
//! - **`payment_schedules/due`** fires once per schedule when the shop's clock passes its due
//!   date: [`tick`] runs before every request of the Admin API and when the clock is moved
//!   (`POST /__lsf/admin/clock`).
//! - **`orderCreateManualPayment`** records a manual `SALE` (gateway `paymentMethodName`, else
//!   "Other", as the argument's description says) of the amount given, else of the outstanding
//!   amount. **`orderCreateMandatePayment`** and **`paymentScheduleCapture`** charge a vaulted
//!   payment method through Shopify Payments: a `SALE` (or an `AUTHORIZATION` when
//!   `autoCapture: false`), answered with a `Job` already `done` and its
//!   `OrderCreateMandatePaymentJobResult` (`SUCCEEDED`), as `orderCancel`'s job is. The same
//!   `idempotencyKey` with the same arguments answers the first answer again. Events:
//!   `order_transactions/create`, `tender_transactions/create`, `orders/paid` once paid in full,
//!   `orders/updated`.
//! - **Mandates**: what `Order.paymentCollectionDetails.vaultedPaymentMethods` lists: the card
//!   of a checkout that left a balance to pay later (lsf vaults it, as Shopify vaults the card of
//!   a deferred purchase), and the customer's payment methods that are not revoked
//!   (`Customer.paymentMethods`, read through the schema's fields). Their ids are made of the
//!   order and the method, so that they are the same every run.
//! - **`paymentReminderSend`** records the reminder email in the shop's outbox
//!   ([`crate::outbox`]): lsf sends nothing.
//! - **Tender transactions**: every successful sale and capture is one (its amount positive),
//!   every refund one (negative), as `tenderTransactions` describes them; each fires
//!   `tender_transactions/create`. The seed makes those of the orders of the store data, without
//!   events.
//!
//! Messages: shopify.dev publishes none for these mutations; each is lsf's wording, in the form
//! of the family's, with the code the payload's error type has.

use chrono::{DateTime, Duration, Utc};
use lsf_core::util::stable_id;
use serde_json::{Map, Value, json};

use super::model::{self, NewTransaction, field_cents, text};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::outbox::{self, Entry};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, gid, parse_gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("orderCreateManualPayment", manual_payment)
        .mutation("orderCreateMandatePayment", mandate_payment)
        .mutation("paymentReminderSend", reminder_send)
        .mutation("paymentScheduleCapture", schedule_capture)
        .mutation("paymentTermsCreate", terms_create)
        .mutation("paymentTermsDelete", terms_delete)
        .mutation("paymentTermsUpdate", terms_update)
        .query("paymentTermsTemplates", templates)
        .field("PaymentTerms", "due", |r, _ctx, object| {
            let now = r.state().now();
            Ok(Output::value(r.map_of(object).is_some_and(|terms| {
                schedules_of(r.state(), terms)
                    .iter()
                    .any(|schedule| is_due(schedule, now))
            })))
        })
        .field("PaymentTerms", "overdue", |r, _ctx, object| {
            let state = r.state();
            Ok(Output::value(r.map_of(object).is_some_and(|terms| {
                schedules_of(state, terms)
                    .iter()
                    .any(|schedule| is_overdue(state, schedule))
            })))
        })
        .field("PaymentSchedule", "due", |r, _ctx, object| {
            let now = r.state().now();
            Ok(Output::value(
                r.map_of(object)
                    .is_some_and(|schedule| is_due(schedule, now)),
            ))
        })
        .field("Order", "paymentCollectionDetails", collection_details)
        // lsf's orders have no duties (no cross-border import): `null`, as for an order that
        // has none.
        .field("Order", "currentTotalDutiesSet", |_r, _ctx, _object| {
            Ok(Output::Null)
        })
        .field("Order", "originalTotalDutiesSet", |_r, _ctx, _object| {
            Ok(Output::Null)
        })
        .field("Order", "refundDiscrepancySet", |r, ctx, object| {
            let state = r.state();
            let cents = r
                .map_of(object)
                .map_or(0, |order| model::refund_discrepancy(state, order));
            r.convert(
                ctx,
                &value::money_bag(cents, &model::currency(state)),
                false,
            )
        })
        // Cash rounding is for cash paid at a point of sale: none on a local shop.
        .field("Order", "totalCashRoundingAdjustment", |r, ctx, _object| {
            let zero = value::money_bag(0, &model::currency(r.state()));
            r.convert(ctx, &json!({"paymentSet": zero, "refundSet": zero}), false)
        })
        .field("OrderTransaction", "paymentId", |r, _ctx, object| {
            Ok(r.map_of(object)
                .and_then(|transaction| payment_id(r.state(), transaction))
                .map_or(Output::Null, Output::value))
        })
        .field(
            "OrderTransaction",
            "maximumRefundableV2",
            |r, ctx, object| {
                let state = r.state();
                let Some(transaction) = r.map_of(object) else {
                    return Ok(Output::Null);
                };
                if !matches!(text(transaction, "kind"), Some("SALE" | "CAPTURE")) {
                    return Ok(Output::Null);
                }
                let id = text(transaction, "id").unwrap_or_default();
                let left = text(transaction, "order")
                    .and_then(|order| state.get(order))
                    .map(|order| model::refundable_payments(state, order))
                    .unwrap_or_default()
                    .into_iter()
                    .find(|(payment, _)| payment == id)
                    .map_or(0, |(_, left)| left);
                r.convert(ctx, &value::money_v2(left, &model::currency(state)), false)
            },
        )
        .field("OrderTransaction", "totalUnsettledSet", |r, ctx, object| {
            let state = r.state();
            let Some(transaction) = r.map_of(object) else {
                return Ok(Output::Null);
            };
            let id = text(transaction, "id").unwrap_or_default();
            let held = if text(transaction, "kind") == Some("AUTHORIZATION") {
                text(transaction, "order")
                    .and_then(|order| state.get(order))
                    .map_or(0, |order| model::capturable_amount(state, order, id))
            } else {
                0
            };
            r.convert(ctx, &value::money_bag(held, &model::currency(state)), false)
        })
        .field(
            "OrderTransaction",
            "shopifyPaymentsSet",
            |r, ctx, object| {
                let Some(transaction) = r.map_of(object) else {
                    return Ok(Output::Null);
                };
                if !super::shopify_payments::processed(transaction) {
                    return Ok(Output::Null);
                }
                let refund = (text(transaction, "kind") == Some("REFUND")).then(|| {
                    let id = text(transaction, "id").unwrap_or_default();
                    // 23 digits, as acquirer reference numbers have, made of the transaction.
                    json!({"acquirerReferenceNumber": format!(
                        "{:020}{:03}",
                        stable_id("acquirer_reference", id),
                        stable_id("acquirer_reference/check", id) % 1000
                    )})
                });
                r.convert(
                    ctx,
                    &json!({"extendedAuthorizationSet": null, "refundSet": refund}),
                    false,
                )
            },
        );
}

/// A transaction's payment id: its order's name and its place among the order's transactions
/// (`#1001.2`), as the examples write it (`#9999.1`).
fn payment_id(state: &ShopState, transaction: &Record) -> Option<String> {
    if let Some(id) = text(transaction, "paymentId") {
        return Some(id.to_string());
    }
    let order = state.get(text(transaction, "order")?)?;
    let id = text(transaction, "id")?;
    let position = gids(order.get("transactions"))
        .iter()
        .position(|known| known == id)?;
    Some(format!("{}.{}", text(order, "name")?, position + 1))
}

/// The templates of `paymentTermsTemplates`' example, in its order: `(id, name, type,
/// dueInDays, description)`.
const TEMPLATES: &[(u64, &str, &str, Option<i64>, &str)] = &[
    (1, "Due on receipt", "RECEIPT", None, "Due on receipt"),
    (
        9,
        "Due on fulfillment",
        "FULFILLMENT",
        None,
        "Due on fulfillment",
    ),
    (2, "Net 7", "NET", Some(7), "Within 7 days"),
    (3, "Net 15", "NET", Some(15), "Within 15 days"),
    (4, "Net 30", "NET", Some(30), "Within 30 days"),
    (8, "Net 45", "NET", Some(45), "Within 45 days"),
    (5, "Net 60", "NET", Some(60), "Within 60 days"),
    (6, "Net 90", "NET", Some(90), "Within 90 days"),
    (7, "Fixed", "FIXED", None, "Fixed date"),
];

/// The gateway of a payment through a vaulted payment method.
const SHOPIFY_PAYMENTS: &str = "shopify_payments";

/// The order field that lists the mandates lsf vaulted for it (the checkout's card).
const MANDATES: &str = "lsfMandates";

/// The schedule field that says `payment_schedules/due` was sent for its due date.
const DUE_SENT: &str = "lsfDueSent";

/// The dates of payment terms and schedules, which their GraphQL types do not have and their
/// webhooks report.
pub const CREATED_AT: &str = "lsfCreatedAt";
pub const UPDATED_AT: &str = "lsfUpdatedAt";

/// Seeds the payment terms templates.
pub fn seed(state: &mut ShopState) {
    for (id, name, kind, days, description) in TEMPLATES {
        state.insert(value::object(json!({
            "__typename": "PaymentTermsTemplate",
            "id": gid("PaymentTermsTemplate", *id),
            "name": name,
            "translatedName": name,
            "paymentTermsType": kind,
            "dueInDays": days,
            "description": description,
        })));
    }
}

/// `paymentTermsTemplates(paymentTermsType:)`.
fn templates(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let kind = ctx.arg_str("paymentTermsType");
    let state = r.state();
    let items: Vec<Value> = state
        .keys("PaymentTermsTemplate")
        .filter(|key| {
            kind.is_none_or(|kind| {
                state
                    .get(key)
                    .and_then(|record| text(record, "paymentTermsType"))
                    == Some(kind)
            })
        })
        .map(|key| json!(key))
        .collect();
    r.convert(ctx, &Value::Array(items), true)
}

// --- Dates ---

/// A `DateTime` of a record or an input.
fn parse_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let text = value?.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// The shop's day of an instant (`2026-03-15`), in its time zone.
fn day_of(state: &ShopState, at: DateTime<Utc>) -> String {
    let shop = state.shop();
    if let Some(zone) = shop
        .and_then(|shop| shop.get("ianaTimezone"))
        .and_then(Value::as_str)
        .and_then(|zone| zone.parse::<chrono_tz::Tz>().ok())
    {
        return at.with_timezone(&zone).format("%Y-%m-%d").to_string();
    }
    at.format("%Y-%m-%d").to_string()
}

/// Whether a schedule is due: not paid, its due date passed.
fn is_due(schedule: &Record, now: DateTime<Utc>) -> bool {
    schedule.get("completedAt").is_none_or(Value::is_null)
        && parse_time(schedule.get("dueAt")).is_some_and(|due| due <= now)
}

/// Whether a schedule is overdue: not paid, the shop's day after its due date.
fn is_overdue(state: &ShopState, schedule: &Record) -> bool {
    schedule.get("completedAt").is_none_or(Value::is_null)
        && parse_time(schedule.get("dueAt"))
            .is_some_and(|due| day_of(state, due) < day_of(state, state.now()))
}

/// The schedules of payment terms, as records.
fn schedules_of<'s>(state: &'s ShopState, terms: &Record) -> Vec<&'s Record> {
    gids(terms.get("paymentSchedules"))
        .iter()
        .filter_map(|schedule| state.get(schedule))
        .collect()
}

// --- Payment terms ---

/// Payment terms to make or change, as read from a `PaymentTermsCreateInput` or a
/// `PaymentTermsInput`.
#[derive(Clone, Debug, PartialEq)]
pub struct TermsSpec {
    pub template: String,
    pub name: String,
    pub kind: String,
    pub days: Option<i64>,
    pub issued_at: Option<DateTime<Utc>>,
    pub due_at: Option<DateTime<Utc>>,
}

/// Reads payment terms from an input (`paymentTermsTemplateId`, `paymentSchedules`): the
/// template must be one of the shop's, at most one schedule, a due date for fixed terms.
/// `base` is the terms being updated: what the input leaves out is theirs. The errors are at
/// `path`, with `code`.
pub fn read_terms(
    state: &ShopState,
    input: &Map<String, Value>,
    path: &[&str],
    code: &str,
    base: Option<&Record>,
) -> Result<TermsSpec, UserError> {
    let at = |field: &'static str| [path, &[field]].concat();
    let template = input
        .get("paymentTermsTemplateId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| base.and_then(|base| text(base, "lsfTemplate").map(str::to_string)))
        .unwrap_or_default();
    let Some(record) = state
        .get(&template)
        .filter(|_| state.type_of(&template) == Some("PaymentTermsTemplate"))
    else {
        return Err(UserError::new(
            &at("paymentTermsTemplateId"),
            "Payment terms template does not exist",
        )
        .code(code));
    };
    let kind = text(record, "paymentTermsType")
        .unwrap_or("NET")
        .to_string();
    let schedules = input
        .get("paymentSchedules")
        .and_then(Value::as_array)
        .cloned();
    if schedules.as_ref().is_some_and(|items| items.len() > 1) {
        return Err(UserError::new(
            &at("paymentSchedules"),
            "Payment terms can have only one payment schedule",
        )
        .code(code));
    }
    let schedule = schedules.as_ref().and_then(|items| items.first());
    let given = |field: &str| -> Result<Option<DateTime<Utc>>, UserError> {
        match schedule.and_then(|schedule| schedule.get(field)) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => parse_time(Some(value)).map(Some).ok_or_else(|| {
                UserError::new(
                    &[path, &["paymentSchedules", "0", field]].concat(),
                    format!("{} is not a valid date", value::humanize(field)),
                )
                .code(code)
            }),
        }
    };
    let (mut issued_at, mut due_at) = (given("issuedAt")?, given("dueAt")?);
    // The base's dates stand for the same kind of terms when no schedule is given.
    if schedules.is_none()
        && let Some(base) = base
        && text(base, "paymentTermsType") == Some(kind.as_str())
        && let Some(first) = gids(base.get("paymentSchedules")).first()
        && let Some(old) = state.get(first)
    {
        issued_at = parse_time(old.get("issuedAt"));
        due_at = parse_time(old.get("dueAt"));
    }
    if kind == "FIXED" && due_at.is_none() {
        return Err(UserError::new(
            &at("paymentSchedules"),
            "A due date is required for fixed payment terms",
        )
        .code(code));
    }
    Ok(TermsSpec {
        name: text(record, "name").unwrap_or_default().to_string(),
        days: record.get("dueInDays").and_then(Value::as_i64),
        template,
        kind,
        issued_at,
        due_at,
    })
}

/// The due date of a schedule of terms issued at `issued_at`.
fn due_date(spec: &TermsSpec, issued_at: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match spec.kind.as_str() {
        "NET" => issued_at.map(|issued| issued + Duration::days(spec.days.unwrap_or(0))),
        "RECEIPT" => issued_at,
        "FIXED" => spec.due_at,
        _ => None,
    }
}

/// Makes the payment terms of an order or a draft order (`reference` is `order` or
/// `draftOrder`, as the field of `PaymentTerms` that names it), with their schedule; the
/// record names them in `paymentTerms`. Net and receipt terms of an order are issued now
/// unless the input says when. Returns the GID of the terms.
pub fn create_terms(
    state: &mut ShopState,
    reference: &str,
    owner: &str,
    spec: &TermsSpec,
) -> String {
    let now = state.now();
    let at = value::date_time(now);
    let terms = state.new_gid("PaymentTerms");
    let schedule = state.new_gid("PaymentSchedule");
    let issued_at = spec.issued_at.or_else(|| {
        (reference == "order" && matches!(spec.kind.as_str(), "NET" | "RECEIPT")).then_some(now)
    });
    let currency = model::currency(state);
    state.insert(value::object(json!({
        "__typename": "PaymentSchedule",
        "id": schedule,
        "paymentTerms": terms,
        "issuedAt": issued_at.map(value::date_time),
        "dueAt": due_date(spec, issued_at).map(value::date_time),
        "completedAt": null,
        "amount": value::money_v2(0, &currency),
        "balanceDue": value::money_v2(0, &currency),
        "totalBalance": value::money_v2(0, &currency),
        CREATED_AT: at,
        UPDATED_AT: at,
    })));
    state.insert(value::object(json!({
        "__typename": "PaymentTerms",
        "id": terms,
        "order": if reference == "order" { json!(owner) } else { Value::Null },
        "draftOrder": if reference == "draftOrder" { json!(owner) } else { Value::Null },
        "paymentTermsName": spec.name,
        "translatedName": spec.name,
        "paymentTermsType": spec.kind,
        "dueInDays": spec.days,
        "paymentSchedules": [schedule],
        "lsfTemplate": spec.template,
        CREATED_AT: at,
        UPDATED_AT: at,
    })));
    if let Some(record) = state.get_mut(owner) {
        record.insert("paymentTerms".into(), json!(terms));
    }
    refresh_terms(state, &terms);
    terms
}

/// Changes payment terms to `spec`: their template and their schedule's dates.
pub fn update_terms(state: &mut ShopState, terms: &str, spec: &TermsSpec) {
    let now = state.timestamp();
    let Some(record) = state.get(terms).cloned() else {
        return;
    };
    let is_order = text(&record, "order").is_some();
    for schedule in gids(record.get("paymentSchedules")) {
        let old_issued = state
            .get(&schedule)
            .and_then(|old| parse_time(old.get("issuedAt")));
        let issued_at = spec.issued_at.or(old_issued).or_else(|| {
            (is_order && matches!(spec.kind.as_str(), "NET" | "RECEIPT")).then(|| state.now())
        });
        let due = due_date(spec, issued_at).map(value::date_time);
        if let Some(record) = state.get_mut(&schedule) {
            if record.get("dueAt").and_then(Value::as_str) != due.as_deref() {
                record.insert(DUE_SENT.into(), json!(false));
            }
            record.insert("issuedAt".into(), json!(issued_at.map(value::date_time)));
            record.insert("dueAt".into(), json!(due));
            record.insert(UPDATED_AT.into(), json!(now));
        }
    }
    if let Some(record) = state.get_mut(terms) {
        record.insert("paymentTermsName".into(), json!(spec.name));
        record.insert("translatedName".into(), json!(spec.name));
        record.insert("paymentTermsType".into(), json!(spec.kind));
        record.insert("dueInDays".into(), json!(spec.days));
        record.insert("lsfTemplate".into(), json!(spec.template));
        record.insert(UPDATED_AT.into(), json!(now));
    }
    refresh_terms(state, terms);
}

/// Issues the schedules of payment terms that were not issued yet (a draft's net terms when
/// its invoice is sent or it is completed): their due date follows.
pub fn issue_terms(state: &mut ShopState, terms: &str) {
    let Some(record) = state.get(terms).cloned() else {
        return;
    };
    let now = state.now();
    let spec = TermsSpec {
        template: text(&record, "lsfTemplate").unwrap_or_default().to_string(),
        name: String::new(),
        kind: text(&record, "paymentTermsType")
            .unwrap_or("NET")
            .to_string(),
        days: record.get("dueInDays").and_then(Value::as_i64),
        issued_at: None,
        due_at: None,
    };
    for schedule in gids(record.get("paymentSchedules")) {
        let Some(old) = state.get(&schedule) else {
            continue;
        };
        if !old.get("issuedAt").is_none_or(Value::is_null)
            || !matches!(spec.kind.as_str(), "NET" | "RECEIPT")
        {
            continue;
        }
        let due = due_date(&spec, Some(now)).map(value::date_time);
        if let Some(record) = state.get_mut(&schedule) {
            record.insert("issuedAt".into(), json!(value::date_time(now)));
            record.insert("dueAt".into(), json!(due));
            record.insert(UPDATED_AT.into(), json!(value::date_time(now)));
        }
    }
}

/// Moves a draft order's payment terms to the order it became: the terms name both, the
/// order names them, their schedule is issued. Emits `payment_terms/update`.
pub fn attach_to_order(state: &mut ShopState, terms: &str, order: &str) {
    if let Some(record) = state.get_mut(terms) {
        record.insert("order".into(), json!(order));
    }
    if let Some(record) = state.get_mut(order) {
        record.insert("paymentTerms".into(), json!(terms));
    }
    issue_terms(state, terms);
    refresh_terms(state, terms);
    state.emit("payment_terms/update", terms);
}

/// Recomputes what the schedules of payment terms derive from their order (or draft order):
/// the amount, the balance due (the order's outstanding amount), the total balance (what is
/// still to be paid or authorized), `completedAt` once nothing is left, and the due date of
/// terms due on fulfillment once the order is fulfilled. Writes only what changed.
pub fn refresh_terms(state: &mut ShopState, terms: &str) {
    let Some(record) = state.get(terms).cloned() else {
        return;
    };
    let currency = model::currency(state);
    let order = text(&record, "order").and_then(|order| state.get(order));
    let draft = text(&record, "draftOrder").and_then(|draft| state.get(draft));
    let (total, balance, to_pay, fulfilled, is_order) = match (order, draft) {
        (Some(order), _) => {
            let outstanding = field_cents(order, "totalOutstandingSet").max(0);
            (
                field_cents(order, "currentTotalPriceSet"),
                outstanding,
                (outstanding - field_cents(order, "totalCapturableSet")).max(0),
                text(order, "displayFulfillmentStatus") == Some("FULFILLED"),
                true,
            )
        }
        (None, Some(draft)) => {
            let total = field_cents(draft, "totalPriceSet");
            (total, total, total, false, false)
        }
        (None, None) => return,
    };
    let now = state.timestamp();
    let on_fulfillment = text(&record, "paymentTermsType") == Some("FULFILLMENT");
    for schedule in gids(record.get("paymentSchedules")) {
        let Some(old) = state.get(&schedule) else {
            continue;
        };
        let mut fields = Map::new();
        for (field, cents) in [
            ("amount", total),
            ("balanceDue", balance),
            ("totalBalance", to_pay),
        ] {
            let money = value::money_v2(cents, &currency);
            if old.get(field) != Some(&money) {
                fields.insert(field.into(), money);
            }
        }
        let completed = !old.get("completedAt").is_none_or(Value::is_null);
        if is_order && balance <= 0 && !completed {
            fields.insert("completedAt".into(), json!(now));
        } else if balance > 0 && completed {
            fields.insert("completedAt".into(), Value::Null);
        }
        if on_fulfillment && fulfilled && old.get("dueAt").is_none_or(Value::is_null) {
            fields.insert("dueAt".into(), json!(now));
        }
        if fields.is_empty() {
            continue;
        }
        fields.insert(UPDATED_AT.into(), json!(now));
        if let Some(record) = state.get_mut(&schedule) {
            record.extend(fields);
        }
    }
}

/// Removes payment terms and their schedules; the record that named them names none. Emits
/// `payment_terms/delete` with the terms as they were.
pub fn delete_terms(state: &mut ShopState, terms: &str) {
    let Some(record) = state.get(terms).cloned() else {
        return;
    };
    for owner in ["order", "draftOrder"] {
        if let Some(owner) = text(&record, owner)
            && let Some(owner) = state.get_mut(owner)
        {
            owner.insert("paymentTerms".into(), Value::Null);
        }
    }
    state.delete(terms, "payment_terms/delete");
    for schedule in gids(record.get("paymentSchedules")) {
        state.remove(&schedule);
    }
}

/// The record payment terms are of (their order, else their draft order), and the topic its
/// change fires.
fn owner_of(record: &Record) -> Option<(String, &'static str)> {
    if let Some(order) = text(record, "order") {
        return Some((order.to_string(), "orders/updated"));
    }
    text(record, "draftOrder").map(|draft| (draft.to_string(), "draft_orders/update"))
}

/// Marks the owner of payment terms changed.
fn owner_changed(state: &mut ShopState, terms: &Record) {
    if let Some((owner, topic)) = owner_of(terms) {
        state.touch(&owner);
        state.emit(topic, &owner);
    }
}

/// `paymentTermsCreate`: payment terms on an order (a draft order's are set with its
/// `paymentTerms` input, as the reference says).
fn terms_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    const CODE: &str = "PAYMENT_TERMS_CREATION_UNSUCCESSFUL";
    let failed = |error: UserError| Ok(payload(json!({"paymentTerms": null}), &[error]));
    let state = mutation.state();
    let reference = ctx.arg_str("referenceId").unwrap_or_default().to_string();
    let Some(order) = state
        .get(&reference)
        .filter(|_| state.type_of(&reference) == Some("Order"))
    else {
        return failed(
            UserError::new(
                &["referenceId"],
                "Reference must be an order: set the payment terms of a draft order with its \
                 `paymentTerms` input",
            )
            .code(CODE),
        );
    };
    if text(order, "paymentTerms").is_some() {
        return failed(
            UserError::new(&["referenceId"], "The order already has payment terms").code(CODE),
        );
    }
    if model::is_cancelled(order) {
        return failed(
            UserError::new(
                &["referenceId"],
                "Payment terms cannot be added to a cancelled order",
            )
            .code(CODE),
        );
    }
    if field_cents(order, "totalOutstandingSet") <= 0 {
        return failed(
            UserError::new(
                &["referenceId"],
                "Payment terms cannot be added to an order that is paid",
            )
            .code(CODE),
        );
    }
    let input = model::input_of(ctx.arg("paymentTermsAttributes"));
    let spec = match read_terms(state, &input, &["paymentTermsAttributes"], CODE, None) {
        Ok(spec) => spec,
        Err(error) => return failed(error),
    };
    let state = mutation.state_mut();
    let terms = create_terms(state, "order", &reference, &spec);
    state.emit("payment_terms/create", &terms);
    model::order_updated(state, &reference);
    Ok(payload(json!({"paymentTerms": terms}), &[]))
}

/// `paymentTermsUpdate`.
fn terms_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    const CODE: &str = "PAYMENT_TERMS_UPDATE_UNSUCCESSFUL";
    let failed = |error: UserError| Ok(payload(json!({"paymentTerms": null}), &[error]));
    let input = model::input_of(ctx.arg("input"));
    let id = input
        .get("paymentTermsId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(record) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("PaymentTerms"))
        .cloned()
    else {
        return failed(
            UserError::new(&["input", "paymentTermsId"], "Payment terms do not exist").code(CODE),
        );
    };
    if schedules_of(state, &record)
        .iter()
        .any(|schedule| !schedule.get("completedAt").is_none_or(Value::is_null))
    {
        return failed(
            UserError::new(
                &["input", "paymentTermsId"],
                "Payment terms that were paid cannot be changed",
            )
            .code(CODE),
        );
    }
    let attributes = model::input_of(input.get("paymentTermsAttributes"));
    let spec = match read_terms(
        state,
        &attributes,
        &["input", "paymentTermsAttributes"],
        CODE,
        Some(&record),
    ) {
        Ok(spec) => spec,
        Err(error) => return failed(error),
    };
    let state = mutation.state_mut();
    update_terms(state, &id, &spec);
    state.emit("payment_terms/update", &id);
    owner_changed(state, &record);
    Ok(payload(json!({"paymentTerms": id}), &[]))
}

/// `paymentTermsDelete`.
fn terms_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx
        .arg("input")
        .and_then(|input| input.get("paymentTermsId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(record) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("PaymentTerms"))
        .cloned()
    else {
        let error = UserError::new(&["input", "paymentTermsId"], "Payment terms do not exist")
            .code("PAYMENT_TERMS_DELETE_UNSUCCESSFUL");
        return Ok(payload(json!({"deletedId": null}), &[error]));
    };
    let state = mutation.state_mut();
    delete_terms(state, &id);
    owner_changed(state, &record);
    Ok(payload(json!({"deletedId": id}), &[]))
}

// --- Payments ---

/// The amount of a payment: the one given (in the shop's currency, above 0, at most what is
/// outstanding), else the outstanding amount. The errors are at `field`.
fn payment_amount(
    state: &ShopState,
    given: Option<(i64, Option<String>)>,
    outstanding: i64,
    field: &str,
) -> Result<i64, (Vec<String>, String, &'static str)> {
    let currency = model::currency(state);
    let Some((amount, given_currency)) = given else {
        return Ok(outstanding);
    };
    let at = |part: &str| vec![field.to_string(), part.to_string()];
    if given_currency.is_some_and(|code| code != currency) {
        return Err((
            at("currencyCode"),
            format!("Currency must be the order's currency ({currency})"),
            "CURRENCY_MISMATCH",
        ));
    }
    if amount <= 0 {
        return Err((
            at("amount"),
            "Amount must be greater than 0".to_string(),
            "AMOUNT_NOT_POSITIVE",
        ));
    }
    if amount > outstanding {
        return Err((
            at("amount"),
            "Amount can't be greater than the outstanding balance of the order".to_string(),
            "AMOUNT_EXCEEDS_BALANCE",
        ));
    }
    Ok(amount)
}

/// What follows a payment recorded on an order: its financial status, `orders/paid` once paid
/// in full, `orders/updated`.
fn after_payment(state: &mut ShopState, order: &str) {
    let before = state
        .get(order)
        .and_then(|record| text(record, "displayFinancialStatus"))
        .map(str::to_string);
    model::settle_financial_status(state, order);
    state.touch(order);
    let paid = state
        .get(order)
        .and_then(|record| text(record, "displayFinancialStatus"))
        == Some("PAID");
    if paid && before.as_deref() != Some("PAID") {
        state.emit("orders/paid", order);
    }
    state.emit("orders/updated", order);
}

/// `orderCreateManualPayment`.
fn manual_payment(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"order": null}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(order) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("Order"))
        .cloned()
    else {
        return failed(&["id"], "Order does not exist", "ORDER_NOT_FOUND");
    };
    let outstanding = field_cents(&order, "totalOutstandingSet");
    if outstanding <= 0 {
        return failed(
            &["id"],
            "The order has no outstanding balance to pay",
            "AMOUNT_EXCEEDS_BALANCE",
        );
    }
    let amount = match payment_amount(
        state,
        model::money_input(ctx.arg("amount")),
        outstanding,
        "amount",
    ) {
        Ok(amount) => amount,
        Err((field, message, code)) => {
            let field: Vec<&str> = field.iter().map(String::as_str).collect();
            return failed(&field, &message, code);
        }
    };
    let processed_at = match ctx.arg("processedAt").filter(|value| !value.is_null()) {
        None => None,
        Some(value) => match parse_time(Some(value)) {
            Some(at) if at <= mutation.now() => Some(value::date_time(at)),
            _ => {
                return failed(
                    &["processedAt"],
                    "Processed at must not be in the future",
                    "PROCESSED_AT_INVALID",
                );
            }
        },
    };
    let gateway = ctx
        .arg_str("paymentMethodName")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Other")
        .to_string();
    let test = order.get("test").and_then(Value::as_bool) == Some(true);
    let state = mutation.state_mut();
    let transaction = model::add_transaction(
        state,
        &id,
        &NewTransaction {
            kind: "SALE",
            amount,
            gateway: Some(&gateway),
            parent: None,
            manual: true,
            test,
        },
    );
    if let Some(at) = processed_at {
        set_processed_at(state, &transaction, &at);
    }
    after_payment(state, &id);
    Ok(payload(json!({"order": id}), &[]))
}

/// Sets when a transaction was processed, and its tender transaction's.
pub fn set_processed_at(state: &mut ShopState, transaction: &str, at: &str) {
    let tender = state
        .get(transaction)
        .and_then(|record| text(record, TENDER))
        .map(str::to_string);
    for key in std::iter::once(transaction.to_string()).chain(tender) {
        if let Some(record) = state.get_mut(&key) {
            record.insert("processedAt".into(), json!(at));
        }
    }
}

/// A vaulted payment method of an order: the GID of its mandate and its `PaymentInstrument`.
struct Mandate {
    id: String,
    instrument: Value,
}

/// The id of the mandate of a payment method (`checkout`, or a `CustomerPaymentMethod`'s GID)
/// for an order.
fn mandate_gid(order: &str, method: &str) -> String {
    gid(
        "PaymentMandate",
        stable_id("payment_mandate", &format!("{order}/{method}")),
    )
}

/// The vaulted payment methods of an order: those lsf vaulted for it ([`vault_checkout_card`]),
/// then its customer's payment methods that are not revoked and that a mandate can charge (a
/// card, a PayPal billing agreement).
fn mandates(state: &ShopState, order: &Record) -> Vec<Mandate> {
    let mut found: Vec<Mandate> = order
        .get(MANDATES)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mandate| {
            Some(Mandate {
                id: mandate.get("id")?.as_str()?.to_string(),
                instrument: mandate.get("paymentInstrument").cloned()?,
            })
        })
        .collect();
    let order_id = text(order, "id").unwrap_or_default();
    let Some(customer) = text(order, "customer").and_then(|customer| state.get(customer)) else {
        return found;
    };
    for method in gids(customer.get("paymentMethods")) {
        let Some(record) = state.get(&method) else {
            continue;
        };
        if !record.get("revokedAt").is_none_or(Value::is_null) {
            continue;
        }
        let instrument = record.get("instrument").cloned().unwrap_or(Value::Null);
        let vaulted = match instrument.get("__typename").and_then(Value::as_str) {
            Some("CustomerCreditCard") => json!({
                "__typename": "VaultCreditCard",
                "brand": instrument.get("brand"),
                "lastDigits": instrument.get("lastDigits"),
                "expiryMonth": instrument.get("expiryMonth"),
                "expiryYear": instrument.get("expiryYear"),
                "name": instrument.get("name"),
                "expired": false,
                "billingAddress": instrument.get("billingAddress"),
            }),
            Some("CustomerPaypalBillingAgreement") => json!({
                "__typename": "VaultPaypalBillingAgreement",
                "inactive": instrument.get("inactive").and_then(Value::as_bool).unwrap_or(false),
                "name": instrument.get("paypalAccountEmail"),
                "paypalAccountEmail": instrument.get("paypalAccountEmail"),
            }),
            _ => continue,
        };
        found.push(Mandate {
            id: mandate_gid(order_id, &method),
            instrument: vaulted,
        });
    }
    found
}

/// Vaults the card a buyer paid with at the local checkout for the balance an order left to
/// pay later (a deposit, a deferred purchase option), as Shopify vaults it: the order's mandate,
/// a card of Shopify's Bogus gateway (`1`, the number its tests charge successfully).
pub fn vault_checkout_card(state: &mut ShopState, order: &str, name: &str) {
    let year = state
        .now()
        .format("%Y")
        .to_string()
        .parse::<i64>()
        .unwrap_or(2026)
        + 4;
    let mandate = json!({
        "id": mandate_gid(order, "checkout"),
        "paymentInstrument": {
            "__typename": "VaultCreditCard",
            "brand": "bogus",
            "lastDigits": "1",
            "expiryMonth": 12,
            "expiryYear": year,
            "name": name,
            "expired": false,
            "billingAddress": null,
        },
    });
    if let Some(record) = state.get_mut(order) {
        record.insert(MANDATES.into(), json!([mandate]));
    }
}

/// `Order.paymentCollectionDetails`: the URL of the order's payment page while money is
/// outstanding *(chosen: Shopify does not document its form)*, and the vaulted payment methods.
fn collection_details(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(order) = r.map_of(object) else {
        return Ok(Output::Null);
    };
    let state = r.state();
    let url = (field_cents(order, "totalOutstandingSet") > 0).then(|| {
        let origin = state
            .shop()
            .and_then(|shop| text(shop, "url"))
            .unwrap_or_default();
        let number = text(order, "id")
            .and_then(parse_gid)
            .map_or(0, |(_, number)| number);
        format!("{origin}/orders/{number}/pay")
    });
    let methods: Vec<Value> = mandates(state, order)
        .into_iter()
        .map(|mandate| {
            json!({"__typename": "PaymentMandate", "id": mandate.id, "paymentInstrument": mandate.instrument})
        })
        .collect();
    Ok(Output::Object(Obj::built(
        "OrderPaymentCollectionDetails",
        value::object(json!({
            "additionalPaymentCollectionUrl": url,
            "vaultedPaymentMethods": methods,
        })),
    )))
}

/// The answer of a payment through a mandate: its `Job`, done, and its job result, which
/// `node` reads.
fn mandate_job(state: &mut ShopState, order: &str, reference: &str) -> Value {
    let result = state.new_gid("OrderCreateMandatePaymentJobResult");
    state.insert(value::object(json!({
        "__typename": "OrderCreateMandatePaymentJobResult",
        "id": result,
        "done": true,
        "status": "SUCCEEDED",
        "errors": [],
        "order": order,
    })));
    let job = crate::resolve::done_job(state, "order_create_mandate_payment");
    json!({
        "job": job,
        "jobResult": result,
        "paymentReferenceId": reference,
    })
}

/// Charges an order through a mandate: a `SALE` (or an `AUTHORIZATION`) through Shopify
/// Payments, then what follows a payment. Returns the payload's fields.
fn charge(state: &mut ShopState, order: &str, amount: i64, capture: bool, key: &str) -> Value {
    let test = state
        .get(order)
        .and_then(|record| record.get("test"))
        .and_then(Value::as_bool)
        == Some(true);
    model::add_transaction(
        state,
        order,
        &NewTransaction {
            kind: if capture { "SALE" } else { "AUTHORIZATION" },
            amount,
            gateway: Some(SHOPIFY_PAYMENTS),
            parent: None,
            manual: false,
            test,
        },
    );
    after_payment(state, order);
    let reference = value::uuid(&format!("{}/{order}/{key}", state.domain()));
    mandate_job(state, order, &reference)
}

/// `orderCreateMandatePayment`.
fn mandate_payment(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    const CODE: &str = "ORDER_MANDATE_PAYMENT_ERROR_CODE";
    let empty = json!({"job": null, "jobResult": null, "paymentReferenceId": null});
    let failed = |field: &[&str], message: &str| {
        Ok(payload(
            empty.clone(),
            &[UserError::new(field, message).code(CODE)],
        ))
    };
    let key = ctx
        .arg_str("idempotencyKey")
        .unwrap_or_default()
        .to_string();
    if key.trim().is_empty() {
        return failed(&["idempotencyKey"], "Idempotency key can't be blank");
    }
    let memo = format!("idempotency/orderCreateMandatePayment/{key}");
    let fingerprint = serde_json::to_string(ctx.arguments()).unwrap_or_default();
    if let Some(remembered) = mutation.state().memo(&memo) {
        if remembered.get("fingerprint").and_then(Value::as_str) == Some(fingerprint.as_str()) {
            return Ok(remembered.get("payload").cloned().unwrap_or(Value::Null));
        }
        return failed(
            &["idempotencyKey"],
            "The same idempotency key cannot be used with different operation parameters.",
        );
    }
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(order) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("Order"))
        .cloned()
    else {
        return failed(&["id"], "Order does not exist");
    };
    let mandate = ctx.arg_str("mandateId").unwrap_or_default();
    if !mandates(state, &order)
        .iter()
        .any(|known| known.id == mandate)
    {
        return failed(
            &["mandateId"],
            "Payment mandate does not exist for this order",
        );
    }
    let mut outstanding = field_cents(&order, "totalOutstandingSet");
    if let Some(schedule) = ctx.arg_str("paymentScheduleId") {
        let terms = text(&order, "paymentTerms").unwrap_or_default();
        let Some(record) = state
            .get(schedule)
            .filter(|record| text(record, "paymentTerms") == Some(terms))
        else {
            return failed(
                &["paymentScheduleId"],
                "Payment schedule does not exist for this order",
            );
        };
        if !record.get("completedAt").is_none_or(Value::is_null) {
            return failed(&["paymentScheduleId"], "Payment schedule is already paid");
        }
        outstanding = outstanding.min(model::cents(record.get("balanceDue")).max(0));
    }
    if outstanding <= 0 {
        return failed(&["id"], "The order has no outstanding balance to pay");
    }
    let amount = match payment_amount(
        state,
        model::money_input(ctx.arg("amount")),
        outstanding,
        "amount",
    ) {
        Ok(amount) => amount,
        Err((field, message, _)) => {
            let field: Vec<&str> = field.iter().map(String::as_str).collect();
            return failed(&field, &message);
        }
    };
    let capture = ctx.arg_bool("autoCapture").unwrap_or(true);
    let state = mutation.state_mut();
    let answer = payload(charge(state, &id, amount, capture, &key), &[]);
    let at = state.timestamp();
    state.set_memo(
        memo,
        json!({"fingerprint": fingerprint, "payload": answer, "at": at}),
    );
    Ok(answer)
}

/// `paymentScheduleCapture`: the balance of a due schedule, through the order's first vaulted
/// payment method.
fn schedule_capture(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |message: &str| {
        Ok(payload(
            json!({"job": null, "jobResult": null, "paymentReferenceId": null}),
            &[UserError::new(&["paymentScheduleId"], message)
                .code("PAYMENT_SCHEDULE_CAPTURE_ERROR_CODE")],
        ))
    };
    let id = ctx
        .arg_str("paymentScheduleId")
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(schedule) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("PaymentSchedule"))
        .cloned()
    else {
        return failed("Payment schedule does not exist");
    };
    let Some(order) = text(&schedule, "paymentTerms")
        .and_then(|terms| state.get(terms))
        .and_then(|terms| text(terms, "order"))
        .and_then(|order| state.get(order))
        .cloned()
    else {
        return failed("Payment schedule is not of an order");
    };
    if !schedule.get("completedAt").is_none_or(Value::is_null) {
        return failed("Payment schedule is already paid");
    }
    if !is_due(&schedule, state.now()) {
        return failed("Payment schedule is not due yet");
    }
    if mandates(state, &order).is_empty() {
        return failed("The order has no vaulted payment method to charge");
    }
    let amount = model::cents(schedule.get("balanceDue"))
        .min(field_cents(&order, "totalOutstandingSet"))
        .max(0);
    if amount <= 0 {
        return failed("Payment schedule has no balance due");
    }
    let order_id = text(&order, "id").unwrap_or_default().to_string();
    let state = mutation.state_mut();
    Ok(payload(charge(state, &order_id, amount, true, &id), &[]))
}

/// `paymentReminderSend`: the reminder is recorded in the shop's outbox.
fn reminder_send(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let failed = |message: &str| {
        Ok(payload(
            json!({"success": false}),
            &[UserError::new(&["paymentScheduleId"], message)
                .code("PAYMENT_REMINDER_SEND_UNSUCCESSFUL")],
        ))
    };
    let id = ctx
        .arg_str("paymentScheduleId")
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(schedule) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("PaymentSchedule"))
        .cloned()
    else {
        return failed("Payment schedule does not exist");
    };
    let Some(order) = text(&schedule, "paymentTerms")
        .and_then(|terms| state.get(terms))
        .and_then(|terms| text(terms, "order"))
        .and_then(|order| state.get(order))
        .cloned()
    else {
        return failed("Payment schedule is not of an order");
    };
    if !schedule.get("completedAt").is_none_or(Value::is_null) {
        return failed("Payment schedule is already paid");
    }
    let Some(email) = text(&order, "email").map(str::to_string) else {
        return failed("The order has no email address to send the reminder to");
    };
    let name = text(&order, "name").unwrap_or_default().to_string();
    let balance = schedule
        .get("balanceDue")
        .and_then(|money| money.get("amount"))
        .and_then(Value::as_str)
        .unwrap_or("0.0")
        .to_string();
    let currency = model::currency(state);
    let due = text(&schedule, "dueAt")
        .and_then(|due| parse_time(Some(&json!(due))))
        .map(|due| day_of(state, due));
    let state = mutation.state_mut();
    let body = match &due {
        Some(due) => format!("{balance} {currency} is due on {due} for order {name}."),
        None => format!("{balance} {currency} is due for order {name}."),
    };
    outbox::record(
        state,
        Entry::email("payment_reminder", &email)
            .subject(format!("Payment reminder for order {name}"))
            .body(body)
            .about(&id),
    );
    Ok(payload(json!({"success": true}), &[]))
}

// --- Tender transactions ---

/// The field of an `OrderTransaction` that names its tender transaction.
pub const TENDER: &str = "lsfTender";

/// Records the tender transaction of an order transaction that moved money: a successful sale
/// or capture (a positive amount), refund or change (a negative one). Returns its GID; `None`
/// for a transaction that moved none (an authorization, a void, a failure).
pub fn record_tender(state: &mut ShopState, transaction: &str) -> Option<String> {
    let record = state.get(transaction)?.clone();
    if text(&record, "status") != Some("SUCCESS") {
        return None;
    }
    let sign = match text(&record, "kind") {
        Some("SALE" | "CAPTURE") => 1,
        Some("REFUND" | "CHANGE") => -1,
        _ => return None,
    };
    let amount = field_cents(&record, "amountSet") * sign;
    let currency = model::currency(state);
    let tender = state.new_gid("TenderTransaction");
    let reference = record
        .get("paymentId")
        .or_else(|| record.get("authorizationCode"))
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| {
            json!(
                parse_gid(transaction)
                    .map_or(0, |(_, number)| number)
                    .to_string()
            )
        });
    state.insert(value::object(json!({
        "__typename": "TenderTransaction",
        "id": tender,
        "order": record.get("order"),
        "amount": value::money_v2(amount, &currency),
        "paymentMethod": record.get("gateway"),
        "processedAt": record.get("processedAt").or_else(|| record.get("createdAt")),
        "remoteReference": reference,
        "test": record.get("test").and_then(Value::as_bool).unwrap_or(false),
        "transactionDetails": null,
        "user": null,
        "lsfTransaction": transaction,
    })));
    if let Some(record) = state.get_mut(transaction) {
        record.insert(TENDER.into(), json!(tender));
    }
    super::shopify_payments::record_balance(state, transaction);
    Some(tender)
}

/// Emits `tender_transactions/create` for the tender transaction of an order transaction.
pub fn emit_tender(state: &mut ShopState, transaction: &str) {
    if let Some(tender) = state
        .get(transaction)
        .and_then(|record| text(record, TENDER))
        .map(str::to_string)
    {
        state.emit("tender_transactions/create", &tender);
    }
}

// --- The clock ---

/// What happens when the shop's clock passes a date: `payment_schedules/due` for each
/// schedule of an order that came due and is not paid, once. Runs before every request of the
/// Admin API, and when the clock is moved.
pub fn tick(state: &mut ShopState) {
    let now = state.now();
    let due: Vec<String> = state
        .keys("PaymentSchedule")
        .filter(|key| {
            state.get(key).is_some_and(|schedule| {
                is_due(schedule, now)
                    && schedule.get(DUE_SENT).and_then(Value::as_bool) != Some(true)
                    && text(schedule, "paymentTerms")
                        .and_then(|terms| state.get(terms))
                        .and_then(|terms| text(terms, "order"))
                        .is_some()
            })
        })
        .map(str::to_string)
        .collect();
    for schedule in due {
        if let Some(record) = state.get_mut(&schedule) {
            record.insert(DUE_SENT.into(), json!(true));
        }
        state.emit("payment_schedules/due", &schedule);
    }
}
