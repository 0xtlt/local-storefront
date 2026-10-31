//! Subscription contracts: how a draft, an atomic creation, a calculation or a checkout becomes a
//! contract ([`commit_new`], [`commit_update`]), the status mutations
//! (`subscriptionContractActivate`, `…Pause`, `…Cancel`, `…Expire`, `…Fail`),
//! `subscriptionContractProductChange`, `subscriptionContractSetNextBillingDate`; the fields and
//! filters of the contracts' queries.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - Activating needs a contract that is "either active, paused, or failed"; a cancelled or
//!   expired contract is terminated: no status mutation changes it again
//!   (`CONTRACT_TERMINATED`, "Subscription contract status cannot be changed once
//!   terminated."). Setting the status a contract has changes nothing and emits nothing.
//! - Every committed change makes a new `revisionId`; a draft or a calculation made from an
//!   older revision is stale (`STALE_CONTRACT`), as Shopify reports a contract "modified since
//!   the calculation was created".
//! - `nextBillingDate` is the app's ("This field is managed by the apps"): lsf sets it as given
//!   and never moves it.
//! - `subscriptionContracts` reads the filters of the reference (`status`, `customer_id`,
//!   `created_at`, `updated_at`, `id`, `last_billing_attempt_error_type`, and `product_id`,
//!   `product_variant_id`, `selling_plan_id` from the lines) and sorts by `CREATED_AT`, `ID`,
//!   `STATUS`, `UPDATED_AT`.
//! - Events: `subscription_contracts/create` and `/update`, and one topic per status:
//!   `subscription_contracts/activate`, `/pause`, `/cancel`, `/expire`, `/fail`.

use serde_json::{Value, json};

use super::cycles;
use super::model::{self, CONTRACT, draft_error, lines, object, refresh, status_error, text};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::search::SearchValue;
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("subscriptionContractActivate", |m, ctx| {
            set_status(m, ctx, "ACTIVE")
        })
        .mutation("subscriptionContractPause", |m, ctx| {
            set_status(m, ctx, "PAUSED")
        })
        .mutation("subscriptionContractCancel", |m, ctx| {
            set_status(m, ctx, "CANCELLED")
        })
        .mutation("subscriptionContractExpire", |m, ctx| {
            set_status(m, ctx, "EXPIRED")
        })
        .mutation("subscriptionContractFail", |m, ctx| {
            set_status(m, ctx, "FAILED")
        })
        .mutation("subscriptionContractProductChange", product_change)
        .mutation(
            "subscriptionContractSetNextBillingDate",
            set_next_billing_date,
        )
        .field("SubscriptionContract", "lineCount", line_count)
        .field(
            "SubscriptionBillingCycleEditedContract",
            "lineCount",
            line_count,
        )
        .field("SubscriptionContract", "groupedLines", grouped_lines)
        .field(
            "SubscriptionBillingCycleEditedContract",
            "groupedLines",
            grouped_lines,
        )
        .field(
            "SubscriptionCalculatedContract",
            "groupedLines",
            grouped_lines,
        )
        .field("Customer", "subscriptionContracts", customer_contracts)
        .search_field(CONTRACT, "product_id", |_r, _item, contract| {
            Some(line_values(contract, "productId"))
        })
        .search_field(CONTRACT, "product_variant_id", |_r, _item, contract| {
            Some(line_values(contract, "variantId"))
        })
        .search_field(CONTRACT, "selling_plan_id", |_r, _item, contract| {
            Some(line_values(contract, "sellingPlanId"))
        });
}

/// The ids a field of a contract's lines names, for a search filter.
fn line_values(contract: &Record, field: &str) -> Vec<SearchValue> {
    lines(contract)
        .iter()
        .filter_map(|line| line.get(field)?.as_str())
        .filter_map(|gid| crate::state::parse_gid(gid).map(|(_, id)| SearchValue::Id(id)))
        .collect()
}

/// The status topic of a status: `subscription_contracts/pause` for `PAUSED`.
pub fn status_topic(status: &str) -> &'static str {
    match status {
        "ACTIVE" => "subscription_contracts/activate",
        "PAUSED" => "subscription_contracts/pause",
        "CANCELLED" => "subscription_contracts/cancel",
        "EXPIRED" => "subscription_contracts/expire",
        _ => "subscription_contracts/fail",
    }
}

/// A contract's code discounts as it keeps them: the codes a draft applied become manual
/// discounts of type `CODE_DISCOUNT` with the code's value (a contract's `discounts` are
/// `SubscriptionManualDiscount`s); a rejected code is dropped.
fn committed_discounts(record: &Record) -> Vec<Value> {
    model::discounts(record)
        .into_iter()
        .filter_map(|discount| {
            if text(discount.as_object()?, "__typename") != Some("SubscriptionAppliedCodeDiscount")
            {
                return Some(discount);
            }
            if discount
                .get("rejectionReason")
                .is_some_and(|reason| !reason.is_null())
            {
                return None;
            }
            let mut manual = discount.get("lsfManual").cloned()?;
            manual["id"] = discount.get("id").cloned().unwrap_or(Value::Null);
            Some(manual)
        })
        .collect()
}

/// The fields a contract takes from a draft, a calculation or an input.
const CONTRACT_FIELDS: [&str; 13] = [
    "customer",
    "customerPaymentMethod",
    "currencyCode",
    "billingPolicy",
    "deliveryPolicy",
    "deliveryMethod",
    "deliveryPrice",
    "lines",
    "discounts",
    "customAttributes",
    "note",
    "nextBillingDate",
    "status",
];

/// Makes a contract of a contract-like record (a draft, a calculated contract, what
/// `subscriptionContractAtomicCreate` or a checkout built): its id, its dates and first revision,
/// its schedule from the shop's clock, its derived amounts. Emits
/// `subscription_contracts/create`. Answers its GID.
pub fn commit_new(state: &mut ShopState, record: &Record, origin_order: Option<&str>) -> String {
    let id = state.new_gid(CONTRACT);
    let now = state.now();
    let timestamp = value::date_time(now);
    let currency = text(record, "currencyCode").unwrap_or("USD").to_string();
    let mut contract = Record::new();
    contract.insert("__typename".into(), json!(CONTRACT));
    contract.insert("id".into(), json!(id));
    for field in CONTRACT_FIELDS {
        contract.insert(
            field.into(),
            record.get(field).cloned().unwrap_or(Value::Null),
        );
    }
    contract.insert(
        "discounts".into(),
        Value::Array(committed_discounts(record)),
    );
    if contract.get("status").is_none_or(Value::is_null) {
        contract.insert("status".into(), json!("ACTIVE"));
    }
    if contract.get("deliveryPrice").is_none_or(Value::is_null) {
        contract.insert("deliveryPrice".into(), model::money(0, &currency));
    }
    for field in ["lines", "customAttributes"] {
        if contract.get(field).is_none_or(Value::is_null) {
            contract.insert(field.into(), json!([]));
        }
    }
    contract.insert("createdAt".into(), json!(timestamp));
    contract.insert("updatedAt".into(), json!(timestamp));
    contract.insert("revisionId".into(), json!("1"));
    contract.insert("originOrder".into(), json!(origin_order));
    contract.insert(
        "orders".into(),
        json!(origin_order.into_iter().collect::<Vec<_>>()),
    );
    contract.insert("billingAttempts".into(), json!([]));
    contract.insert("app".into(), json!(state.keys("App").next()));
    contract.insert("appAdminUrl".into(), Value::Null);
    contract.insert("lastPaymentStatus".into(), Value::Null);
    contract.insert("lastBillingAttemptErrorType".into(), Value::Null);
    let policy = contract
        .get("billingPolicy")
        .cloned()
        .unwrap_or(Value::Null);
    contract.insert(
        "lsfSchedule".into(),
        cycles::new_schedule(state, &policy, now),
    );
    contract.insert("lsfCycleEdits".into(), json!({}));
    refresh(state, &mut contract);
    state.insert(contract);
    state.emit("subscription_contracts/create", &id);
    id
}

/// Puts what a draft or a calculation changed of a contract into it: the fields it gives, a new
/// schedule when the billing policy changed, a new revision. Emits
/// `subscription_contracts/update`, then the status topic when the status changed.
pub fn commit_update(state: &mut ShopState, contract: &str, record: &Record) {
    let Some(mut current) = state.get(contract).cloned() else {
        return;
    };
    let before_status = text(&current, "status").map(str::to_string);
    let before_policy = current.get("billingPolicy").cloned();
    for field in CONTRACT_FIELDS {
        if field == "customer" || field == "currencyCode" {
            continue;
        }
        if let Some(value) = record.get(field) {
            current.insert(field.into(), value.clone());
        }
    }
    current.insert(
        "discounts".into(),
        Value::Array(committed_discounts(record)),
    );
    if current.get("status").is_none_or(Value::is_null) {
        current.insert("status".into(), json!(before_status));
    }
    if current.get("billingPolicy") != before_policy.as_ref() {
        let policy = current.get("billingPolicy").cloned().unwrap_or(Value::Null);
        cycles::reschedule(state, &mut current, &policy);
    }
    refresh(state, &mut current);
    state.insert(current);
    model::bump(state, contract);
    state.emit("subscription_contracts/update", contract);
    let after = state
        .get(contract)
        .and_then(|record| text(record, "status"))
        .map(str::to_string);
    if after != before_status
        && let Some(after) = after
    {
        state.emit(status_topic(&after), contract);
    }
}

/// The contract of an argument, or the error of the field.
pub fn contract_arg(state: &ShopState, id: &str) -> Option<Record> {
    state
        .get(id)
        .filter(|_| state.type_of(id) == Some(CONTRACT))
        .cloned()
}

/// Whether a contract is cancelled or expired.
pub fn is_terminated(contract: &Record) -> bool {
    model::TERMINATED.contains(&text(contract, "status").unwrap_or_default())
}

/// The status mutations: `subscriptionContractActivate` (from `ACTIVE`, `PAUSED` or `FAILED`),
/// `…Pause`, `…Cancel`, `…Expire`, `…Fail`, each with its topic.
fn set_status(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    status: &str,
) -> Result<Value, FieldError> {
    let id = ctx
        .arg_str("subscriptionContractId")
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(contract) = contract_arg(state, &id) else {
        return Ok(payload(
            json!({"contract": null}),
            &[
                UserError::new(&["subscriptionContractId"], "Contract does not exist")
                    .code("INVALID"),
            ],
        ));
    };
    if is_terminated(&contract) && text(&contract, "status") != Some(status) {
        return Ok(payload(
            json!({"contract": null}),
            &[status_error(
                &["subscriptionContractId"],
                "CONTRACT_TERMINATED",
            )],
        ));
    }
    if text(&contract, "status") != Some(status) {
        let state = mutation.state_mut();
        if let Some(record) = state.get_mut(&id) {
            record.insert("status".into(), json!(status));
        }
        model::bump(state, &id);
        state.emit(status_topic(status), &id);
    }
    Ok(payload(json!({"contract": id}), &[]))
}

/// `subscriptionContractProductChange`: a line's variant (its product, titles, SKU, image) and
/// price, on the contract itself, as a new revision; `lineUpdated` is the line. Emits
/// `subscription_contracts/update`.
fn product_change(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"contract": null, "lineUpdated": null});
    let id = ctx
        .arg_str("subscriptionContractId")
        .unwrap_or_default()
        .to_string();
    let line_id = ctx.arg_str("lineId").unwrap_or_default().to_string();
    let input = object(ctx.arg("input"));
    let state = mutation.state();
    let Some(mut contract) = contract_arg(state, &id) else {
        return Ok(payload(
            empty,
            &[
                UserError::new(&["subscriptionContractId"], "Contract does not exist")
                    .code("INVALID"),
            ],
        ));
    };
    if cycles::has_future_contract_edits(state, &contract) {
        return Ok(payload(
            empty,
            &[draft_error(&["subscriptionContractId"], "HAS_FUTURE_EDITS")],
        ));
    }
    let currency = text(&contract, "currencyCode").unwrap_or("USD").to_string();
    let mut all = lines(&contract);
    let Some(line) = all
        .iter_mut()
        .find(|line| line.get("id").and_then(Value::as_str) == Some(line_id.as_str()))
    else {
        return Ok(payload(
            empty,
            &[UserError::new(&["lineId"], "Line does not exist").code("INVALID")],
        ));
    };
    if let Err(errors) = model::update_line(state, line, &input, &currency, &["input"]) {
        return Ok(payload(empty, &errors));
    }
    // A variant given without a price takes the variant's.
    if input.contains_key("productVariantId")
        && input.get("currentPrice").is_none_or(Value::is_null)
        && let Some(variant) = text(&input, "productVariantId")
        && let Some(fields) = model::variant_fields(state, variant)
    {
        let price = fields.get("lsfPrice").and_then(Value::as_i64).unwrap_or(0);
        line["currentPrice"] = model::money(price, &currency);
    }
    contract.insert("lines".into(), Value::Array(all));
    refresh(state, &mut contract);
    let updated = model::line_of(&contract, &line_id)
        .cloned()
        .unwrap_or(Value::Null);
    let state = mutation.state_mut();
    state.insert(contract);
    model::bump(state, &id);
    state.emit("subscription_contracts/update", &id);
    Ok(payload(
        json!({"contract": id, "lineUpdated": updated}),
        &[],
    ))
}

/// `subscriptionContractSetNextBillingDate`: the app's next billing date. Emits
/// `subscription_contracts/update`.
fn set_next_billing_date(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("contractId").unwrap_or_default().to_string();
    let state = mutation.state();
    if contract_arg(state, &id).is_none() {
        return Ok(payload(
            json!({"contract": null}),
            &[UserError::new(&["contractId"], "Contract does not exist").code("INVALID")],
        ));
    }
    let Some(date) = ctx.arg_str("date").and_then(model::date_time) else {
        return Ok(payload(
            json!({"contract": null}),
            &[UserError::new(&["date"], "Date is invalid").code("INVALID")],
        ));
    };
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("nextBillingDate".into(), json!(value::date_time(date)));
    }
    model::bump(state, &id);
    state.emit("subscription_contracts/update", &id);
    Ok(payload(json!({"contract": id}), &[]))
}

// ---- fields ---------------------------------------------------------------------------------

/// `lineCount` (deprecated): how many lines.
fn line_count(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(Output::value(r.items_of(object, "lines").len()))
}

/// `groupedLines`: the lines, none of them a bundle's parent (lsf makes no bundles).
fn grouped_lines(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items: Vec<Value> = r
        .items_of(object, "lines")
        .into_iter()
        .map(|mut line| {
            line["__typename"] = json!("SubscriptionLine");
            line
        })
        .collect();
    r.connection(ctx, items)
}

/// `Customer.subscriptionContracts`.
fn customer_contracts(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let customer = r
        .map_of(object)
        .and_then(|record| text(record, "id"))
        .unwrap_or_default();
    let items: Vec<Value> = r
        .state()
        .all(CONTRACT)
        .filter(|contract| text(contract, "customer") == Some(customer))
        .filter_map(|contract| contract.get("id").cloned())
        .collect();
    r.connection(ctx, items)
}
