//! The draft API, deprecated in 2026-10 for the calculation API but still served:
//! `subscriptionContractCreate`, `subscriptionContractUpdate`, `subscriptionDraftUpdate`,
//! `subscriptionDraftLineAdd`, `…LineUpdate`, `…LineRemove`, `subscriptionDraftDiscountAdd`,
//! `…DiscountUpdate`, `…DiscountRemove`, `…DiscountCodeApply`,
//! `subscriptionDraftFreeShippingDiscountAdd`, `…FreeShippingDiscountUpdate`,
//! `subscriptionDraftCommit`; `subscriptionContractAtomicCreate`; the drafts of one billing
//! cycle: `subscriptionBillingCycleContractEdit`, `subscriptionBillingCycleContractDraftCommit`,
//! `subscriptionBillingCycleContractDraftConcatenate`. And the fields of a draft that compare it
//! with where it started (`linesAdded`, `discountsUpdated`, …), its `billingCycle`, its
//! `deliveryOptions`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `subscriptionContractCreate` makes a draft, "an intention to create a new subscription";
//!   nothing is a contract until `subscriptionDraftCommit`, which emits
//!   `subscription_contracts/create`. `subscriptionContractUpdate` makes a draft of an existing
//!   contract, which changes nothing "until committed" (`subscription_contracts/update`).
//! - A committed draft is done (`COMMITTED`); a draft of an older revision is stale
//!   (`STALE_CONTRACT`); a contract with a contract edit on its current or a future cycle cannot
//!   be updated (`HAS_FUTURE_EDITS`).
//! - A draft of a billing cycle commits with `subscriptionBillingCycleContractDraftCommit`
//!   (`BILLING_CYCLE_PRESENT` with `subscriptionDraftCommit`, `BILLING_CYCLE_ABSENT` the other
//!   way), cannot change the policies (`BILLING_CYCLE_CONTRACT_DRAFT_BILLING_POLICY_INVALID`,
//!   `…DELIVERY_POLICY_INVALID`), and edits that cycle only: `subscription_billing_cycle_edits
//!   /create` or `/update`. Concatenating merges the lines of other contracts' cycles into it
//!   (each line's `concatenatedOriginContract`); billing the cycle bills them all in one order.
//! - A code (`subscriptionDraftDiscountCodeApply`) is one of the shop's code discounts: applied
//!   when it is active and applies on subscriptions, else kept with its `rejectionReason`
//!   (`NOT_FOUND`, `CURRENTLY_INACTIVE`, `INCOMPATIBLE_PURCHASE_TYPE`) and dropped on commit.
//! - Messages: the descriptions of `SubscriptionDraftErrorCode`; those of a draft or a line that
//!   is not one ("Draft does not exist") are lsf's.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use super::contracts::{commit_new, commit_update, contract_arg};
use super::cycles;
use super::model::{
    self, DRAFT, Pointer, draft_error, lines, manual_discount, object, objects, refresh, text,
};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("subscriptionContractCreate", contract_create)
        .mutation("subscriptionContractUpdate", contract_update)
        .mutation("subscriptionDraftUpdate", draft_update)
        .mutation("subscriptionDraftLineAdd", line_add)
        .mutation("subscriptionDraftLineUpdate", line_update)
        .mutation("subscriptionDraftLineRemove", line_remove)
        .mutation("subscriptionDraftDiscountAdd", discount_add)
        .mutation("subscriptionDraftDiscountUpdate", discount_update)
        .mutation("subscriptionDraftDiscountRemove", discount_remove)
        .mutation("subscriptionDraftDiscountCodeApply", discount_code_apply)
        .mutation(
            "subscriptionDraftFreeShippingDiscountAdd",
            free_shipping_add,
        )
        .mutation(
            "subscriptionDraftFreeShippingDiscountUpdate",
            free_shipping_update,
        )
        .mutation("subscriptionDraftCommit", draft_commit)
        .mutation("subscriptionContractAtomicCreate", atomic_create)
        .mutation("subscriptionBillingCycleContractEdit", cycle_contract_edit)
        .mutation(
            "subscriptionBillingCycleContractDraftCommit",
            cycle_draft_commit,
        )
        .mutation(
            "subscriptionBillingCycleContractDraftConcatenate",
            cycle_draft_concatenate,
        )
        .field(DRAFT, "linesAdded", |r, ctx, object| {
            changed_items(r, ctx, object, "lines", Change::Added)
        })
        .field(DRAFT, "linesRemoved", |r, ctx, object| {
            changed_items(r, ctx, object, "lines", Change::Removed)
        })
        .field(DRAFT, "discountsAdded", |r, ctx, object| {
            changed_items(r, ctx, object, "discounts", Change::Added)
        })
        .field(DRAFT, "discountsRemoved", |r, ctx, object| {
            changed_items(r, ctx, object, "discounts", Change::Removed)
        })
        .field(DRAFT, "discountsUpdated", |r, ctx, object| {
            changed_items(r, ctx, object, "discounts", Change::Updated)
        })
        .field(DRAFT, "billingCycle", draft_billing_cycle)
        .field(DRAFT, "concatenatedBillingCycles", concatenated_cycles)
        .field(DRAFT, "deliveryOptions", |r, ctx, object| {
            delivery_options(r, ctx, object, false)
        })
        .field(DRAFT, "shippingOptions", |r, ctx, object| {
            delivery_options(r, ctx, object, true)
        });
}

/// The most contracts a cycle draft concatenates (`EXCEEDED_MAX_CONCATENATED_CONTRACTS`): the 50
/// of the calculation API's concatenation, which shopify.dev documents.
const MAX_CONCATENATED: usize = 50;

/// Marks the shop changed for a draft, which no webhook reports.
fn changed(state: &mut ShopState, draft: &str) {
    state.emit(crate::domains::app::CHANGED, draft);
}

/// A new draft of a contract-like record.
fn insert_draft(
    state: &mut ShopState,
    mut record: Record,
    original: Option<&str>,
    cycle: Option<Value>,
) -> String {
    let id = state.new_gid(DRAFT);
    record.insert("__typename".into(), json!(DRAFT));
    record.insert("id".into(), json!(id));
    record.insert("originalContract".into(), json!(original));
    record.insert(
        "lsfOriginal".into(),
        json!({"lines": lines(&record), "discounts": model::discounts(&record)}),
    );
    if let Some(original) = original {
        record.insert(
            "lsfRevision".into(),
            json!(model::revision(state, original)),
        );
    }
    record.insert("lsfCycle".into(), cycle.unwrap_or(Value::Null));
    record.insert("lsfConcatenated".into(), json!([]));
    record.insert("lsfCommitted".into(), json!(false));
    for field in [
        "createdAt",
        "updatedAt",
        "revisionId",
        "orders",
        "billingAttempts",
        "originOrder",
        "lsfSchedule",
        "lsfCycleEdits",
        "latestCommittedProjectedOrderTotals",
        "lastPaymentStatus",
        "lastBillingAttemptErrorType",
        "app",
        "appAdminUrl",
    ] {
        record.remove(field);
    }
    state.insert(record);
    changed(state, &id);
    id
}

/// The draft of `draftId`, not committed, or the error.
fn open_draft(state: &ShopState, ctx: &FieldContext<'_>) -> Result<(String, Record), UserError> {
    let id = ctx.arg_str("draftId").unwrap_or_default().to_string();
    let Some(draft) = state.get_of(&id, DRAFT) else {
        return Err(model::invalid_error(&["draftId"], "Draft does not exist"));
    };
    if draft.get("lsfCommitted").and_then(Value::as_bool) == Some(true) {
        return Err(draft_error(&["draftId"], "COMMITTED"));
    }
    Ok((id, draft.clone()))
}

/// Saves a draft after a change, its amounts derived again.
fn save(state: &mut ShopState, id: &str, mut draft: Record) {
    refresh(state, &mut draft);
    state.insert(draft);
    changed(state, id);
}

/// `subscriptionContractCreate`: a draft of a new contract.
fn contract_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = object(ctx.arg("input"));
    let mut errors = Vec::new();
    let Some((customer, currency, next)) = new_contract(mutation.state(), &input, &mut errors)
    else {
        return Ok(payload(json!({"draft": null}), &errors));
    };
    let mut record = base_record(&customer, &currency);
    record.insert("nextBillingDate".into(), json!(value::date_time(next)));
    let contract = object(input.get("contract"));
    let state = mutation.state_mut();
    model::apply_draft_input(
        state,
        &mut record,
        &contract,
        &["input", "contract"],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"draft": null}), &errors));
    }
    refresh(state, &mut record);
    let id = insert_draft(state, record, None, None);
    Ok(payload(json!({"draft": id}), &[]))
}

/// The customer, currency and next billing date of the input of a new contract, checked.
fn new_contract(
    state: &ShopState,
    input: &Map<String, Value>,
    errors: &mut Vec<UserError>,
) -> Option<(String, String, DateTime<Utc>)> {
    let customer = text(input, "customerId").unwrap_or_default().to_string();
    if state.type_of(&customer) != Some("Customer") {
        errors.push(draft_error(
            &["input", "customerId"],
            "CUSTOMER_DOES_NOT_EXIST",
        ));
    }
    let currency = text(input, "currencyCode").unwrap_or_default().to_string();
    if !model::currency_enabled(state, &currency) {
        errors.push(draft_error(
            &["input", "currencyCode"],
            "CURRENCY_NOT_ENABLED",
        ));
    }
    let next = text(input, "nextBillingDate").and_then(model::date_time);
    if next.is_none() {
        errors.push(draft_error(
            &["input", "nextBillingDate"],
            "INVALID_BILLING_DATE",
        ));
    }
    match next {
        Some(next) if errors.is_empty() => Some((customer, currency, next)),
        _ => None,
    }
}

/// The fields of a new contract-like record of a customer.
pub fn base_record(customer: &str, currency: &str) -> Record {
    value::object(json!({
        "status": "ACTIVE",
        "customer": customer,
        "customerPaymentMethod": null,
        "currencyCode": currency,
        "billingPolicy": null,
        "deliveryPolicy": null,
        "deliveryMethod": null,
        "deliveryPrice": model::money(0, currency),
        "lines": [],
        "discounts": [],
        "customAttributes": [],
        "note": null,
    }))
}

/// `subscriptionContractUpdate`: a draft of an existing contract.
fn contract_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("contractId").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(contract) = contract_arg(state, &id) else {
        return Ok(payload(
            json!({"draft": null}),
            &[model::invalid_error(
                &["contractId"],
                "Contract does not exist",
            )],
        ));
    };
    if cycles::has_future_contract_edits(state, &contract) {
        return Ok(payload(
            json!({"draft": null}),
            &[draft_error(&["contractId"], "HAS_FUTURE_EDITS")],
        ));
    }
    let mut record = contract.clone();
    record.remove("id");
    let draft = insert_draft(mutation.state_mut(), record, Some(&id), None);
    Ok(payload(json!({"draft": draft}), &[]))
}

/// `subscriptionDraftUpdate`: the contract-level fields of a draft.
fn draft_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let (id, mut draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"draft": null}), &[error])),
    };
    let input = object(ctx.arg("input"));
    let mut errors = Vec::new();
    if draft.get("lsfCycle").is_some_and(|cycle| !cycle.is_null()) {
        if input
            .get("billingPolicy")
            .is_some_and(|value| !value.is_null())
        {
            errors.push(draft_error(
                &["input", "billingPolicy"],
                "BILLING_CYCLE_CONTRACT_DRAFT_BILLING_POLICY_INVALID",
            ));
        }
        if input
            .get("deliveryPolicy")
            .is_some_and(|value| !value.is_null())
        {
            errors.push(draft_error(
                &["input", "deliveryPolicy"],
                "BILLING_CYCLE_CONTRACT_DRAFT_DELIVERY_POLICY_INVALID",
            ));
        }
    }
    let state = mutation.state_mut();
    model::apply_draft_input(state, &mut draft, &input, &["input"], &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"draft": null}), &errors));
    }
    save(state, &id, draft);
    Ok(payload(json!({"draft": id}), &[]))
}

/// `subscriptionDraftLineAdd`.
fn line_add(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"draft": null, "lineAdded": null});
    let (id, mut draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let input = object(ctx.arg("input"));
    let currency = text(&draft, "currencyCode").unwrap_or("USD").to_string();
    if lines(&draft).len() >= model::MAX_LINES {
        return Ok(payload(empty, &[draft_error(&["input"], "TOO_MANY_LINES")]));
    }
    let state = mutation.state_mut();
    let line = match model::new_line(state, &input, &currency, &["input"]) {
        Ok(line) => line,
        Err(errors) => return Ok(payload(empty, &errors)),
    };
    let line_id = line
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut all = lines(&draft);
    all.push(line);
    draft.insert("lines".into(), Value::Array(all));
    save(state, &id, draft);
    let added = state
        .get(&id)
        .and_then(|draft| model::line_of(draft, &line_id).cloned())
        .unwrap_or(Value::Null);
    Ok(payload(json!({"draft": id, "lineAdded": added}), &[]))
}

/// `subscriptionDraftLineUpdate`: a patch of a line.
fn line_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"draft": null, "lineUpdated": null});
    let (id, mut draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let line_id = ctx.arg_str("lineId").unwrap_or_default().to_string();
    let input = object(ctx.arg("input"));
    let currency = text(&draft, "currencyCode").unwrap_or("USD").to_string();
    let mut all = lines(&draft);
    let Some(line) = all
        .iter_mut()
        .find(|line| line.get("id").and_then(Value::as_str) == Some(line_id.as_str()))
    else {
        return Ok(payload(
            empty,
            &[model::invalid_error(&["lineId"], "Line does not exist")],
        ));
    };
    if let Err(errors) = model::update_line(mutation.state(), line, &input, &currency, &["input"]) {
        return Ok(payload(empty, &errors));
    }
    draft.insert("lines".into(), Value::Array(all));
    let state = mutation.state_mut();
    save(state, &id, draft);
    let updated = state
        .get(&id)
        .and_then(|draft| model::line_of(draft, &line_id).cloned())
        .unwrap_or(Value::Null);
    Ok(payload(json!({"draft": id, "lineUpdated": updated}), &[]))
}

/// `subscriptionDraftLineRemove`: the line, and its place in the discounts that listed it
/// (`discountsUpdated`).
fn line_remove(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"draft": null, "lineRemoved": null, "discountsUpdated": null});
    let (id, mut draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let line_id = ctx.arg_str("lineId").unwrap_or_default().to_string();
    let mut all = lines(&draft);
    let Some(position) = all
        .iter()
        .position(|line| line.get("id").and_then(Value::as_str) == Some(line_id.as_str()))
    else {
        let was = draft
            .pointer("/lsfOriginal/lines")
            .and_then(Value::as_array)
            .is_some_and(|lines| {
                lines
                    .iter()
                    .any(|line| line.get("id").and_then(Value::as_str) == Some(line_id.as_str()))
            });
        let error = if was {
            draft_error(&["lineId"], "ALREADY_REMOVED")
        } else {
            model::invalid_error(&["lineId"], "Line does not exist")
        };
        return Ok(payload(empty, &[error]));
    };
    let removed = all.remove(position);
    let mut updated = Vec::new();
    let discounts: Vec<Value> = model::discounts(&draft)
        .into_iter()
        .map(|mut discount| {
            let listed = gids(discount.pointer("/entitledLines/lsfLineIds"));
            if listed.contains(&line_id) {
                let kept: Vec<String> = listed
                    .into_iter()
                    .filter(|known| *known != line_id)
                    .collect();
                discount["entitledLines"]["lsfLineIds"] = json!(kept);
                updated.push(discount["id"].clone());
            }
            discount
        })
        .collect();
    draft.insert("lines".into(), Value::Array(all));
    draft.insert("discounts".into(), Value::Array(discounts));
    let state = mutation.state_mut();
    save(state, &id, draft);
    let saved = state.get(&id).cloned().unwrap_or_default();
    let updated: Vec<Value> = model::discounts(&saved)
        .into_iter()
        .filter(|discount| updated.contains(&discount["id"]))
        .collect();
    Ok(payload(
        json!({"draft": id, "lineRemoved": removed, "discountsUpdated": updated}),
        &[],
    ))
}

/// The lines a `SubscriptionManualDiscountEntitledLinesInput` entitles, from those it had: `all`,
/// or the lines `add`ed less those `remove`d. Errors for a line the draft does not have.
fn entitled_lines(
    draft: &Record,
    input: &Map<String, Value>,
    previous: (bool, Vec<String>),
    errors: &mut Vec<UserError>,
) -> (bool, Vec<String>) {
    let Some(entitled) = input.get("entitledLines").and_then(Value::as_object) else {
        return previous;
    };
    let (mut all, mut listed) = previous;
    if let Some(given) = entitled.get("all").and_then(Value::as_bool) {
        all = given;
    }
    let lines = object(entitled.get("lines"));
    let known: Vec<String> = model::lines(draft)
        .iter()
        .filter_map(|line| line.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    for (index, line) in gids(lines.get("add")).into_iter().enumerate() {
        if !known.contains(&line) {
            let index = index.to_string();
            errors.push(model::invalid_error(
                &["input", "entitledLines", "lines", "add", &index],
                "Line does not exist",
            ));
        } else if !listed.contains(&line) {
            listed.push(line);
            if !entitled.contains_key("all") {
                all = false;
            }
        }
    }
    let removed = gids(lines.get("remove"));
    listed.retain(|line| !removed.contains(line));
    if !all && listed.is_empty() {
        errors.push(draft_error(
            &["input", "entitledLines"],
            "NO_ENTITLED_LINES",
        ));
    }
    (all, listed)
}

/// `subscriptionDraftDiscountAdd`: a manual discount on the lines.
fn discount_add(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"draft": null, "discountAdded": null});
    let (id, mut draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let input = object(ctx.arg("input"));
    let currency = text(&draft, "currencyCode").unwrap_or("USD").to_string();
    let mut errors = Vec::new();
    let value = model::discount_value(&input, &currency);
    if value.is_none() {
        errors.push(draft_error(&["input", "value"], "PRESENCE"));
    }
    check_discount_value(&input, &mut errors);
    let entitled = entitled_lines(&draft, &input, (true, Vec::new()), &mut errors);
    if model::discounts(&draft).len() >= model::MAX_DISCOUNTS {
        errors.push(draft_error(&["input"], "TOO_MANY_DISCOUNTS"));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let discount_id = model::uuid_gid(state, "SubscriptionManualDiscount");
    let discount = manual_discount(
        discount_id.clone(),
        text(&input, "title"),
        "LINE_ITEM",
        value.unwrap_or(Value::Null),
        input.get("recurringCycleLimit").and_then(Value::as_i64),
        "MANUAL",
        entitled,
    );
    let mut all = model::discounts(&draft);
    all.push(discount);
    draft.insert("discounts".into(), Value::Array(all));
    save(state, &id, draft);
    let added = discount_of(state, &id, &discount_id);
    Ok(payload(json!({"draft": id, "discountAdded": added}), &[]))
}

/// The errors of a discount's value: a percentage from 0 to 100, an amount of at least 0, a
/// cycle limit of at least 0.
fn check_discount_value(input: &Map<String, Value>, errors: &mut Vec<UserError>) {
    let value = object(input.get("value"));
    if let Some(percentage) = value.get("percentage").and_then(Value::as_i64)
        && !(0..=100).contains(&percentage)
    {
        errors.push(draft_error(
            &["input", "value", "percentage"],
            "NOT_IN_RANGE",
        ));
    }
    if let Some(amount) = value
        .get("fixedAmount")
        .and_then(|fixed| fixed.get("amount"))
        .and_then(Value::as_f64)
        && amount < 0.0
    {
        errors.push(draft_error(
            &["input", "value", "fixedAmount", "amount"],
            "GREATER_THAN_OR_EQUAL_TO",
        ));
    }
    if input
        .get("recurringCycleLimit")
        .and_then(Value::as_i64)
        .is_some_and(|limit| limit < 0)
    {
        errors.push(draft_error(
            &["input", "recurringCycleLimit"],
            "GREATER_THAN_OR_EQUAL_TO",
        ));
    }
}

/// A discount of a draft as it is saved.
fn discount_of(state: &ShopState, draft: &str, discount: &str) -> Value {
    state
        .get(draft)
        .map(model::discounts)
        .unwrap_or_default()
        .into_iter()
        .find(|known| known.get("id").and_then(Value::as_str) == Some(discount))
        .unwrap_or(Value::Null)
}

/// The position of a discount in a draft, or the error of `discountId`.
fn discount_position(draft: &Record, ctx: &FieldContext<'_>) -> Result<(String, usize), UserError> {
    let id = ctx.arg_str("discountId").unwrap_or_default().to_string();
    model::discounts(draft)
        .iter()
        .position(|discount| discount.get("id").and_then(Value::as_str) == Some(id.as_str()))
        .map(|position| (id, position))
        .ok_or_else(|| model::invalid_error(&["discountId"], "Discount does not exist"))
}

/// `subscriptionDraftDiscountUpdate` and `subscriptionDraftFreeShippingDiscountUpdate`: a patch
/// of a manual discount (on the lines, or on the delivery).
fn update_discount(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    shipping: bool,
) -> Result<Value, FieldError> {
    let empty = json!({"draft": null, "discountUpdated": null});
    let (id, mut draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let (discount_id, position) = match discount_position(&draft, ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let input = object(ctx.arg("input"));
    let currency = text(&draft, "currencyCode").unwrap_or("USD").to_string();
    let mut all = model::discounts(&draft);
    let discount = &mut all[position];
    let target = discount
        .get("targetType")
        .and_then(Value::as_str)
        .unwrap_or("LINE_ITEM");
    if (target == "SHIPPING_LINE") != shipping
        || discount.get("__typename").and_then(Value::as_str) != Some("SubscriptionManualDiscount")
    {
        return Ok(payload(
            empty,
            &[model::invalid_error(
                &["discountId"],
                "Discount is not of this kind",
            )],
        ));
    }
    let mut errors = Vec::new();
    check_discount_value(&input, &mut errors);
    if !shipping {
        let previous = (
            discount
                .pointer("/entitledLines/all")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            gids(discount.pointer("/entitledLines/lsfLineIds")),
        );
        let (all_lines, listed) = entitled_lines(&draft, &input, previous, &mut errors);
        discount["entitledLines"]["all"] = json!(all_lines);
        discount["entitledLines"]["lsfLineIds"] = json!(listed);
        if input.contains_key("value") {
            match model::discount_value(&input, &currency) {
                Some(value) => discount["value"] = value,
                None => errors.push(draft_error(&["input", "value"], "PRESENCE")),
            }
        }
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    for field in ["title", "recurringCycleLimit"] {
        if let Some(given) = input.get(field) {
            discount[field] = given.clone();
        }
    }
    draft.insert("discounts".into(), Value::Array(all));
    let state = mutation.state_mut();
    save(state, &id, draft);
    let updated = discount_of(state, &id, &discount_id);
    Ok(payload(
        json!({"draft": id, "discountUpdated": updated}),
        &[],
    ))
}

fn discount_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    update_discount(mutation, ctx, false)
}

fn free_shipping_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    update_discount(mutation, ctx, true)
}

/// `subscriptionDraftDiscountRemove`.
fn discount_remove(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"draft": null, "discountRemoved": null});
    let (id, mut draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let (_, position) = match discount_position(&draft, ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let mut all = model::discounts(&draft);
    let removed = all.remove(position);
    draft.insert("discounts".into(), Value::Array(all));
    save(mutation.state_mut(), &id, draft);
    Ok(payload(
        json!({"draft": id, "discountRemoved": removed}),
        &[],
    ))
}

/// `subscriptionDraftFreeShippingDiscountAdd`: all of the delivery off.
fn free_shipping_add(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"draft": null, "discountAdded": null});
    let (id, mut draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let input = object(ctx.arg("input"));
    let mut errors = Vec::new();
    check_discount_value(&input, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let discount_id = model::uuid_gid(state, "SubscriptionManualDiscount");
    let discount = manual_discount(
        discount_id.clone(),
        text(&input, "title"),
        "SHIPPING_LINE",
        json!({"__typename": "SubscriptionDiscountPercentageValue", "percentage": 100}),
        input.get("recurringCycleLimit").and_then(Value::as_i64),
        "MANUAL",
        (true, Vec::new()),
    );
    let mut all = model::discounts(&draft);
    all.push(discount);
    draft.insert("discounts".into(), Value::Array(all));
    save(state, &id, draft);
    let added = discount_of(state, &id, &discount_id);
    Ok(payload(json!({"draft": id, "discountAdded": added}), &[]))
}

/// The shop's code discount of a code, and why it would not apply to a subscription: `None`
/// when it applies.
fn code_discount(state: &ShopState, code: &str) -> Result<(Record, Option<&'static str>), ()> {
    let now = state.timestamp();
    let node = state.all("DiscountCodeNode").find(|node| {
        crate::domains::app::discounts::inner(node)
            .and_then(|discount| discount.get("codes"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("code").and_then(Value::as_str))
            .any(|known| known.eq_ignore_ascii_case(code.trim()))
    });
    let discount = node
        .and_then(crate::domains::app::discounts::inner)
        .ok_or(())?;
    let reason = if crate::domains::app::discounts::status_at(discount, &now) != "ACTIVE" {
        Some("CURRENTLY_INACTIVE")
    } else if discount
        .pointer("/customerGets/appliesOnSubscription")
        .and_then(Value::as_bool)
        == Some(false)
    {
        Some("INCOMPATIBLE_PURCHASE_TYPE")
    } else {
        None
    };
    Ok((discount.clone(), reason))
}

/// The manual discount a code discount gives a contract: its value (a percentage, an amount, or
/// free shipping), on all the lines.
fn code_as_manual(discount: &Record, id: &str, code: &str, currency: &str) -> Value {
    let shipping = text(discount, "__typename").is_some_and(|kind| kind.contains("FreeShipping"));
    let gets = discount
        .pointer("/customerGets/value")
        .cloned()
        .unwrap_or(Value::Null);
    let value = if shipping {
        json!({"__typename": "SubscriptionDiscountPercentageValue", "percentage": 100})
    } else if let Some(percentage) = gets.get("percentage").and_then(Value::as_f64) {
        // Shopify's `DiscountPercentage` is a fraction (`0.1`); a subscription's is a whole
        // percentage.
        json!({"__typename": "SubscriptionDiscountPercentageValue", "percentage": (percentage * 100.0).round() as i64})
    } else {
        json!({
            "__typename": "SubscriptionDiscountFixedAmountValue",
            "amount": model::money(crate::state::value::cents(gets.get("amount")), currency),
            "appliesOnEachItem": gets.get("appliesOnEachItem").and_then(Value::as_bool).unwrap_or(false),
        })
    };
    let title = text(discount, "title").unwrap_or(code);
    manual_discount(
        id.to_string(),
        Some(title),
        if shipping {
            "SHIPPING_LINE"
        } else {
            "LINE_ITEM"
        },
        value,
        discount.get("recurringCycleLimit").and_then(Value::as_i64),
        "CODE_DISCOUNT",
        (true, Vec::new()),
    )
}

/// A `SubscriptionAppliedCodeDiscount` of a code, with the manual discount it becomes on commit.
pub fn applied_code(state: &mut ShopState, code: &str, currency: &str) -> Value {
    let id = model::uuid_gid(state, "SubscriptionAppliedCodeDiscount");
    let (reason, manual) = match code_discount(state, code) {
        Err(()) => (Some("NOT_FOUND"), Value::Null),
        Ok((discount, reason)) => (reason, code_as_manual(&discount, &id, code, currency)),
    };
    json!({
        "__typename": "SubscriptionAppliedCodeDiscount",
        "id": id,
        "redeemCode": code.trim(),
        "rejectionReason": reason,
        "lsfManual": manual,
    })
}

/// `subscriptionDraftDiscountCodeApply`.
fn discount_code_apply(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"draft": null, "appliedDiscount": null});
    let (id, mut draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let code = ctx.arg_str("redeemCode").unwrap_or_default().to_string();
    if code.trim().is_empty() {
        return Ok(payload(empty, &[draft_error(&["redeemCode"], "BLANK")]));
    }
    let currency = text(&draft, "currencyCode").unwrap_or("USD").to_string();
    let state = mutation.state_mut();
    let applied = applied_code(state, &code, &currency);
    let mut all = model::discounts(&draft);
    all.push(applied.clone());
    draft.insert("discounts".into(), Value::Array(all));
    save(state, &id, draft);
    Ok(payload(
        json!({"draft": id, "appliedDiscount": applied}),
        &[],
    ))
}

/// `subscriptionDraftCommit`: a new contract, or the changes of the contract the draft is of.
fn draft_commit(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let (id, draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"contract": null}), &[error])),
    };
    let state = mutation.state();
    if draft.get("lsfCycle").is_some_and(|cycle| !cycle.is_null()) {
        return Ok(payload(
            json!({"contract": null}),
            &[draft_error(&["draftId"], "BILLING_CYCLE_PRESENT")],
        ));
    }
    let mut errors = Vec::new();
    model::check_committable(&draft, &["draftId"], &mut errors);
    if let Some(error) = customer_check(state, &draft) {
        errors.push(error);
    }
    let original = text(&draft, "originalContract").map(str::to_string);
    if let Some(original) = &original {
        match contract_arg(state, original) {
            Some(contract) => {
                if text(&draft, "lsfRevision") != Some(model::revision(state, original).as_str()) {
                    errors.push(draft_error(&["draftId"], "STALE_CONTRACT"));
                } else if cycles::has_future_contract_edits(state, &contract) {
                    errors.push(draft_error(&["draftId"], "HAS_FUTURE_EDITS"));
                }
            }
            None => errors.push(draft_error(&["draftId"], "INVALID")),
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"contract": null}), &errors));
    }
    let state = mutation.state_mut();
    let contract = match &original {
        Some(original) => {
            commit_update(state, original, &draft);
            original.clone()
        }
        None => commit_new(state, &draft, None),
    };
    if let Some(record) = state.get_mut(&id) {
        record.insert("lsfCommitted".into(), json!(true));
    }
    Ok(payload(json!({"contract": contract}), &[]))
}

/// The error of a contract-like record whose customer is gone, or whose payment method is not
/// the customer's.
fn customer_check(state: &ShopState, record: &Record) -> Option<UserError> {
    let customer = text(record, "customer").unwrap_or_default();
    if state.type_of(customer) != Some("Customer") {
        return Some(draft_error(&["draftId"], "CUSTOMER_DOES_NOT_EXIST"));
    }
    let method = text(record, "customerPaymentMethod")?;
    model::check_payment_method(state, method, Some(customer), &["draftId"])
}

/// `subscriptionContractAtomicCreate`: a contract at once, from its draft input, its lines
/// (each with its discounts) and its codes.
fn atomic_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = object(ctx.arg("input"));
    let mut errors = Vec::new();
    let Some((customer, currency, next)) = new_contract(mutation.state(), &input, &mut errors)
    else {
        return Ok(payload(json!({"contract": null}), &errors));
    };
    let mut record = base_record(&customer, &currency);
    record.insert("nextBillingDate".into(), json!(value::date_time(next)));
    let contract_input = object(input.get("contract"));
    let state = mutation.state_mut();
    model::apply_draft_input(
        state,
        &mut record,
        &contract_input,
        &["input", "contract"],
        &mut errors,
    );
    let mut all_lines = Vec::new();
    let mut all_discounts = Vec::new();
    for (index, line_input) in objects(input.get("lines")).iter().enumerate() {
        let index = index.to_string();
        let line = object(line_input.get("line"));
        match model::new_line(state, &line, &currency, &["input", "lines", &index, "line"]) {
            Ok(line) => {
                let line_id = line
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                for discount in objects(line_input.get("discounts")) {
                    let Some(value) = model::discount_value(&discount, &currency) else {
                        errors.push(draft_error(
                            &["input", "lines", &index, "discounts", "value"],
                            "PRESENCE",
                        ));
                        continue;
                    };
                    let id = model::uuid_gid(state, "SubscriptionManualDiscount");
                    all_discounts.push(manual_discount(
                        id,
                        text(&discount, "title"),
                        "LINE_ITEM",
                        value,
                        discount.get("recurringCycleLimit").and_then(Value::as_i64),
                        "MANUAL",
                        (false, vec![line_id.clone()]),
                    ));
                }
                all_lines.push(line);
            }
            Err(more) => errors.extend(more),
        }
    }
    for code in gids(input.get("discountCodes")) {
        all_discounts.push(applied_code(state, &code, &currency));
    }
    record.insert("lines".into(), Value::Array(all_lines));
    record.insert("discounts".into(), Value::Array(all_discounts));
    model::check_committable(&record, &["input"], &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"contract": null}), &errors));
    }
    let contract = commit_new(state, &record, None);
    Ok(payload(json!({"contract": contract}), &[]))
}

// ---- billing cycle drafts -------------------------------------------------------------------

/// The contract of a billing cycle as it bills: its edit's contract, else the contract's own
/// fields.
pub fn cycle_contract(contract: &Record, index: i64) -> Record {
    match cycles::edits(contract, index)
        .and_then(|edits| edits.get("contract"))
        .and_then(Value::as_object)
    {
        Some(edited) => edited.clone(),
        None => contract.clone(),
    }
}

/// `subscriptionBillingCycleContractEdit`: a draft of one cycle's contract.
fn cycle_contract_edit(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = object(ctx.arg("billingCycleInput"));
    let state = mutation.state();
    let id = text(&input, "contractId").unwrap_or_default().to_string();
    let Some(contract) = contract_arg(state, &id) else {
        return Ok(payload(
            json!({"draft": null}),
            &[model::invalid_error(
                &["billingCycleInput", "contractId"],
                "Contract does not exist",
            )],
        ));
    };
    let cycle = match cycles::select(state, &contract, &object(input.get("selector"))) {
        Ok(cycle) => cycle,
        Err(miss) => {
            return Ok(payload(
                json!({"draft": null}),
                &[draft_error(
                    &["billingCycleInput", "selector"],
                    miss.draft_code(),
                )],
            ));
        }
    };
    if cycles::is_billed(state, &contract, cycle.index) {
        return Ok(payload(
            json!({"draft": null}),
            &[model::invalid_error(
                &["billingCycleInput", "selector"],
                "Billing cycle has already been billed",
            )],
        ));
    }
    let mut record = cycle_contract(&contract, cycle.index);
    record.insert("__typename".into(), json!(DRAFT));
    record.remove("id");
    for field in [
        "customer",
        "currencyCode",
        "billingPolicy",
        "deliveryPolicy",
        "status",
        "nextBillingDate",
    ] {
        record.insert(
            field.into(),
            contract.get(field).cloned().unwrap_or(Value::Null),
        );
    }
    let draft = insert_draft(
        mutation.state_mut(),
        record,
        Some(&id),
        Some(json!({"contract": id, "index": cycle.index})),
    );
    Ok(payload(json!({"draft": draft}), &[]))
}

/// `subscriptionBillingCycleContractDraftConcatenate`: the lines of other contracts' cycles
/// merged into a cycle draft.
fn cycle_draft_concatenate(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let (id, mut draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"draft": null}), &[error])),
    };
    let state = mutation.state();
    let Some(anchor) = draft
        .get("lsfCycle")
        .filter(|cycle| !cycle.is_null())
        .cloned()
    else {
        let code = if text(&draft, "originalContract").is_none() {
            "CONCATENATION_UNCOMMITTED_CONTRACT_DRAFT"
        } else {
            "CONCATENATION_BILLING_CYCLE_CONTRACT_DRAFT_REQUIRED"
        };
        return Ok(payload(
            json!({"draft": null}),
            &[draft_error(&["draftId"], code)],
        ));
    };
    let anchor_contract = anchor
        .get("contract")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let inputs = objects(ctx.arg("concatenatedBillingCycleContracts"));
    let mut errors = Vec::new();
    let mut concatenated: Vec<Value> = draft
        .get("lsfConcatenated")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut merged = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let index = index.to_string();
        let at = ["concatenatedBillingCycleContracts", index.as_str()];
        let contract_id = text(input, "contractId").unwrap_or_default().to_string();
        let Some(contract) = contract_arg(state, &contract_id) else {
            errors.push(model::invalid_error(
                &model::path(&at, &["contractId"]),
                "Contract does not exist",
            ));
            continue;
        };
        let already = contract_id == anchor_contract
            || concatenated.iter().any(|known| {
                known.get("contract").and_then(Value::as_str) == Some(contract_id.as_str())
            });
        if already {
            errors.push(draft_error(
                &model::path(&at, &["contractId"]),
                "DUPLICATE_CONCATENATED_CONTRACTS",
            ));
            continue;
        }
        if text(&contract, "customer") != text(&draft, "customer") {
            errors.push(draft_error(
                &model::path(&at, &["contractId"]),
                "CUSTOMER_MISMATCH",
            ));
            continue;
        }
        if text(&contract, "currencyCode") != text(&draft, "currencyCode") {
            // lsf's wording: one order bills the group, in one currency.
            errors.push(model::invalid_error(
                &model::path(&at, &["contractId"]),
                "Currency must be the draft's",
            ));
            continue;
        }
        let cycle = match cycles::select(state, &contract, &object(input.get("selector"))) {
            Ok(cycle) => cycle,
            Err(miss) => {
                errors.push(draft_error(
                    &model::path(&at, &["selector"]),
                    miss.draft_code(),
                ));
                continue;
            }
        };
        concatenated.push(json!({"contract": contract_id, "index": cycle.index}));
        merged.push((contract_id, cycle_contract(&contract, cycle.index)));
    }
    if concatenated.len() > MAX_CONCATENATED {
        errors.push(draft_error(
            &["concatenatedBillingCycleContracts"],
            "EXCEEDED_MAX_CONCATENATED_CONTRACTS",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"draft": null}), &errors));
    }
    let state = mutation.state_mut();
    let mut all = lines(&draft);
    for (contract_id, contract) in merged {
        for mut line in lines(&contract) {
            line["id"] = json!(model::uuid_gid(state, "SubscriptionLine"));
            line["concatenatedOriginContract"] = json!(contract_id);
            all.push(line);
        }
    }
    draft.insert("lines".into(), Value::Array(all));
    draft.insert("lsfConcatenated".into(), Value::Array(concatenated));
    save(state, &id, draft);
    Ok(payload(json!({"draft": id}), &[]))
}

/// An edited contract of a cycle: what `SubscriptionBillingCycleEditedContract` reads.
pub fn edited_contract(state: &ShopState, record: &Record, contract: &str, index: i64) -> Value {
    let mut edited = Record::new();
    edited.insert(
        "__typename".into(),
        json!("SubscriptionBillingCycleEditedContract"),
    );
    for field in [
        "customer",
        "customerPaymentMethod",
        "currencyCode",
        "deliveryMethod",
        "deliveryPrice",
        "lines",
        "discounts",
        "customAttributes",
        "note",
    ] {
        edited.insert(
            field.into(),
            record.get(field).cloned().unwrap_or(Value::Null),
        );
    }
    let now = state.timestamp();
    edited.insert("createdAt".into(), json!(now));
    edited.insert("updatedAt".into(), json!(now));
    edited.insert("app".into(), json!(state.app_gid()));
    edited.insert("appAdminUrl".into(), Value::Null);
    edited.insert("orders".into(), json!([]));
    edited.insert("lsfContract".into(), json!(contract));
    edited.insert("lsfCycleIndex".into(), json!(index));
    refresh(state, &mut edited);
    let totals = model::projected_totals(state, &edited);
    edited.insert("latestCommittedProjectedOrderTotals".into(), totals);
    Value::Object(edited)
}

/// Commits the contract of one cycle: the edit of the anchor (and of the concatenated cycles,
/// which bill with it), with `subscription_billing_cycle_edits/create` or `/update`. Answers the
/// edited contract.
pub fn commit_cycle_edit(
    state: &mut ShopState,
    contract: &str,
    index: i64,
    record: &Record,
    concatenated: &[Value],
) -> Value {
    let edited = edited_contract(state, record, contract, index);
    let topic = cycles::edit_topic(state, contract, index);
    if let Some(record) = state.get_mut(contract) {
        cycles::set_edit(record, index, "contract", Some(edited.clone()));
        cycles::set_edit(
            record,
            index,
            "concatenated",
            (!concatenated.is_empty()).then(|| Value::Array(concatenated.to_vec())),
        );
    }
    cycles::emit_cycle(state, topic, contract, index);
    for member in concatenated {
        let member_contract = member
            .get("contract")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let member_index = member.get("index").and_then(Value::as_i64).unwrap_or(0);
        let topic = cycles::edit_topic(state, member_contract, member_index);
        if let Some(record) = state.get_mut(member_contract) {
            cycles::set_edit(
                record,
                member_index,
                "concatenatedInto",
                Some(json!({"contract": contract, "index": index})),
            );
        }
        cycles::emit_cycle(state, topic, member_contract, member_index);
    }
    edited
}

/// `subscriptionBillingCycleContractDraftCommit`.
fn cycle_draft_commit(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let (id, draft) = match open_draft(mutation.state(), ctx) {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"contract": null}), &[error])),
    };
    let Some(cycle) = draft
        .get("lsfCycle")
        .filter(|cycle| !cycle.is_null())
        .cloned()
    else {
        return Ok(payload(
            json!({"contract": null}),
            &[draft_error(&["draftId"], "BILLING_CYCLE_ABSENT")],
        ));
    };
    let contract = cycle
        .get("contract")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let index = cycle.get("index").and_then(Value::as_i64).unwrap_or(0);
    let state = mutation.state();
    let mut errors = Vec::new();
    model::check_committable(&draft, &["draftId"], &mut errors);
    if let Some(error) = customer_check(state, &draft) {
        errors.push(error);
    }
    match contract_arg(state, &contract) {
        Some(record) if cycles::is_billed(state, &record, index) => {
            errors.push(model::invalid_error(
                &["draftId"],
                "Billing cycle has already been billed",
            ));
        }
        Some(_) => {}
        None => errors.push(draft_error(&["draftId"], "INVALID")),
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"contract": null}), &errors));
    }
    let concatenated = draft
        .get("lsfConcatenated")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let state = mutation.state_mut();
    let edited = commit_cycle_edit(state, &contract, index, &draft, &concatenated);
    if let Some(record) = state.get_mut(&id) {
        record.insert("lsfCommitted".into(), json!(true));
    }
    Ok(payload(json!({"contract": edited}), &[]))
}

// ---- fields of a draft ----------------------------------------------------------------------

/// What a draft field compares with where the draft started.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    Added,
    Removed,
    Updated,
}

/// `linesAdded`, `linesRemoved`, `discountsAdded`, `discountsRemoved`, `discountsUpdated`.
fn changed_items(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
    field: &str,
    change: Change,
) -> Result<Output<Obj>, FieldError> {
    let Some(draft) = r.map_of(object) else {
        return r.connection(ctx, Vec::new());
    };
    let now: Vec<Value> = draft
        .get(field)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let before: Vec<Value> = draft
        .get("lsfOriginal")
        .and_then(|original| original.get(field))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    /// The item of `items` with the id of `item`.
    fn find<'i>(items: &'i [Value], item: &Value) -> Option<&'i Value> {
        let id = item.get("id").and_then(Value::as_str);
        items
            .iter()
            .find(|known| known.get("id").and_then(Value::as_str) == id)
    }
    let items: Vec<Value> = match change {
        Change::Added => now
            .iter()
            .filter(|item| find(&before, item).is_none())
            .cloned()
            .collect(),
        Change::Removed => before
            .iter()
            .filter(|item| find(&now, item).is_none())
            .cloned()
            .collect(),
        Change::Updated => now
            .iter()
            .filter(|item| find(&before, item).is_some_and(|old| strip(old) != strip(item)))
            .cloned()
            .collect(),
    };
    r.connection(ctx, items)
}

/// A discount without what its lines derive: what changes when it is updated.
fn strip(discount: &Value) -> Value {
    let mut discount = discount.clone();
    if let Some(entitled) = discount
        .get_mut("entitledLines")
        .and_then(Value::as_object_mut)
    {
        entitled.remove("lines");
    }
    discount
}

/// `SubscriptionDraft.billingCycle`: the cycle a cycle draft edits.
fn draft_billing_cycle(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(cycle) = r
        .map_of(object)
        .and_then(|draft| draft.get("lsfCycle"))
        .filter(|cycle| !cycle.is_null())
    else {
        return Ok(Output::Null);
    };
    let contract = cycle
        .get("contract")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let index = cycle.get("index").and_then(Value::as_i64).unwrap_or(0);
    let state = r.state();
    let Some(record) = contract_arg(state, contract) else {
        return Ok(Output::Null);
    };
    match cycles::cycle(state, &record, index) {
        Some(cycle) => r.convert(
            ctx,
            &cycles::to_value(state, contract, &record, &cycle),
            false,
        ),
        None => Ok(Output::Null),
    }
}

/// `SubscriptionDraft.concatenatedBillingCycles`.
fn concatenated_cycles(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let items: Vec<Value> = r
        .map_of(object)
        .and_then(|draft| draft.get("lsfConcatenated"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|member| {
            let contract = member.get("contract")?.as_str()?;
            let index = member.get("index")?.as_i64()?;
            let record = contract_arg(state, contract)?;
            let cycle = cycles::cycle(state, &record, index)?;
            Some(cycles::to_value(state, contract, &record, &cycle))
        })
        .collect();
    r.connection(ctx, items)
}

/// The delivery options of the store data's checkout, for an address: shipping rates as
/// `SubscriptionShippingOption`s, pickup as `SubscriptionPickupOption`s at the first location
/// with pickup, local delivery as `SubscriptionLocalDeliveryOption`s. The rates are the same for
/// every address, as the local checkout's.
pub fn rate_options(
    state: &ShopState,
    currency: &str,
    shipping_only: bool,
    calculated: bool,
) -> Vec<Value> {
    let pickup_location = state
        .all("Location")
        .find(|location| {
            location
                .get("localPickupSettingsV2")
                .is_some_and(|settings| !settings.is_null())
        })
        .and_then(|location| text(location, "id").map(str::to_string));
    let prefix = if calculated {
        "SubscriptionContractCalculation"
    } else {
        "Subscription"
    };
    state
        .setting(super::DELIVERY_RATES_SETTING)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|rate| {
            let kind = rate.get("kind").and_then(Value::as_str).unwrap_or("shipping");
            if shipping_only && kind != "shipping" {
                return None;
            }
            let mut option = json!({
                "code": rate.get("handle"),
                "title": rate.get("title"),
                "description": rate.get("description"),
                "presentmentTitle": rate.get("title"),
                "phoneRequired": kind == "local",
                "price": model::money(rate.get("price").and_then(Value::as_i64).unwrap_or(0), currency),
            });
            match kind {
                "pickup" => {
                    option["__typename"] = json!(format!("{prefix}PickupOption"));
                    option["location"] = json!(pickup_location.clone()?);
                    option["pickupTime"] = json!("Usually ready in 24 hours");
                }
                "local" => option["__typename"] = json!(format!("{prefix}LocalDeliveryOption")),
                _ => option["__typename"] = json!(format!("{prefix}ShippingOption")),
            }
            Some(option)
        })
        .collect()
}

/// `SubscriptionDraft.deliveryOptions(deliveryAddress:)` and the deprecated `shippingOptions`:
/// the store data's rates, for the address given or the draft's.
fn delivery_options(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
    shipping_only: bool,
) -> Result<Output<Obj>, FieldError> {
    let Some(draft) = r.map_of(object) else {
        return Ok(Output::Null);
    };
    let address = ctx
        .arg("deliveryAddress")
        .filter(|value| !value.is_null())
        .or_else(|| model::delivery_address(draft));
    let currency = text(draft, "currencyCode").unwrap_or("USD");
    let result = if address.is_none() {
        let kind = if shipping_only {
            "SubscriptionShippingOptionResultFailure"
        } else {
            "SubscriptionDeliveryOptionResultFailure"
        };
        // lsf's wording.
        json!({"__typename": kind, "message": "A delivery address is required"})
    } else if shipping_only {
        json!({
            "__typename": "SubscriptionShippingOptionResultSuccess",
            "shippingOptions": rate_options(r.state(), currency, true, false),
        })
    } else {
        json!({
            "__typename": "SubscriptionDeliveryOptionResultSuccess",
            "deliveryOptions": rate_options(r.state(), currency, false, false),
        })
    };
    r.convert(ctx, &result, false)
}
