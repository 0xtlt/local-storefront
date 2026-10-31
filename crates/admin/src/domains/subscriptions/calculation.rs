//! The calculation API of 2026-10: `subscriptionContractCreateCalculate`,
//! `subscriptionContractUpdateCalculate`, `subscriptionBillingCycleContractEditCalculate`,
//! `subscriptionContractCalculationCommit`, and the `subscriptionContractCalculation` query.
//!
//! As shopify.dev's "Migrate to the SubscriptionContractCalculation API" describes it:
//!
//! - A calculate mutation takes the desired state of the contract and answers a calculation
//!   `SubscriptionContractCalculationPending`; polling `subscriptionContractCalculation` then
//!   reads a `…Success` (the calculated contract, its projected order totals, its warnings, the
//!   delivery options) or a `…Failure` (its errors). lsf calculates within the request, so the
//!   first poll reads the outcome. Nothing changes until `subscriptionContractCalculationCommit`.
//! - On an update "omitted top-level fields are preserved, but a list you provide (such as
//!   `lines`) replaces the stored list", and each line is "a complete restatement": a line
//!   without `priceOverride` is priced from its variant, one without
//!   `appManagedPricingPolicy` loses its policy. An `id` keeps a line's identity; an id the
//!   contract does not have is refused.
//! - The delivery price is the method's (`deliveryPrice`, required on `shipping`,
//!   `localDelivery`, `pickup`); `none` is no delivery; `fetchAvailableDeliveryOptions` only
//!   discovers the options (the store data's shipping rates) and cannot be committed
//!   (`DISCOVERY_CALCULATION_NOT_COMMITTABLE`).
//! - Discounts: `manualDiscounts` (`orderDiscount` on the lines, `deliveryDiscount` on the
//!   delivery, `recurringCycleLimit: 0` for no limit), the lines' own `discounts`, and
//!   `discountCodes` (a code that does not apply is a `DISCOUNT_CODE_NOT_APPLIED` warning).
//! - Failures (`SubscriptionContractCalculationDiagnosticCode`): `MERCHANDISE_NOT_FOUND`,
//!   `PAYMENT_METHOD_NOT_FOUND`, `CURRENCY_NOT_SUPPORTED`, `DELIVERY_METHOD_REQUIRED`;
//!   warnings: `DELIVERY_METHOD_UNUSED`, `DISCOUNT_CODE_NOT_APPLIED`.
//! - Commit: a create makes the contract (`subscription_contracts/create`), an update applies
//!   the changes (`subscription_contracts/update`), a cycle edit edits that cycle only
//!   (`subscription_billing_cycle_edits/create` or `/update`, the answer a
//!   `SubscriptionBillingCycleEditedContract`). Refused: a calculation that failed
//!   (`NOT_READY_TO_COMMIT`), of a contract changed since (`STALE_CONTRACT`) or terminated
//!   (`CONTRACT_TERMINATED`).
//! - The webhooks `subscription_contract_calculations/succeed` and `/fail` the guide names are
//!   not topics of 2026-10's `WebhookSubscriptionTopic`: none is sent. Bundles
//!   (`productVariantParentLine`) are kept as a line of their parent variant: lsf runs no cart
//!   transform on a contract.

use serde_json::{Map, Value, json};

use super::contracts::{commit_new, commit_update, contract_arg, is_terminated};
use super::cycles;
use super::drafts::{applied_code, commit_cycle_edit, rate_options};
use super::model::{
    self, CALCULATION, Pointer, lines, manual_discount, object, objects, refresh, text,
};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("subscriptionContractCreateCalculate", create_calculate)
        .mutation("subscriptionContractUpdateCalculate", update_calculate)
        .mutation(
            "subscriptionBillingCycleContractEditCalculate",
            cycle_edit_calculate,
        )
        .mutation("subscriptionContractCalculationCommit", commit)
        .query("subscriptionContractCalculation", |r, _ctx, _object| {
            let id = _ctx.arg_str("id").unwrap_or_default();
            Ok(Output::object_or_null(
                r.reference(id, "SubscriptionContractCalculation"),
            ))
        });
}

/// What a calculation is of.
enum Target {
    Create,
    Update(String),
    Cycle(String, i64),
}

/// A diagnostic of a calculation.
fn diagnostic(code: &str, field: &[&str]) -> Value {
    json!({
        "code": code,
        "field": field,
        "message": model::described("SubscriptionContractCalculationDiagnosticCode", code),
    })
}

/// A line of the calculation API's `SubscriptionContractCalculationLineInput` (`@oneOf`
/// `productVariantLine`, `customLine`, `productVariantParentLine`), with its own discounts; or
/// the diagnostic of a variant that is not one, or the user error of a line id the contract
/// does not have.
#[allow(clippy::too_many_arguments)]
fn calculated_line(
    state: &mut ShopState,
    input: &Map<String, Value>,
    currency: &str,
    existing: &[Value],
    path: &[&str],
    diagnostics: &mut Vec<Value>,
    errors: &mut Vec<UserError>,
    discounts: &mut Vec<Value>,
) -> Option<Value> {
    let (kind, line) = [
        "productVariantLine",
        "customLine",
        "productVariantParentLine",
    ]
    .iter()
    .find_map(|kind| {
        input
            .get(*kind)
            .and_then(Value::as_object)
            .map(|line| (*kind, line))
    })?;
    let quantity = line.get("quantity").and_then(Value::as_i64).unwrap_or(0);
    if quantity < 1 {
        errors.push(
            UserError::new(
                &model::path(path, &[kind, "quantity"]),
                "Quantity must be greater than 0",
            )
            .code("INVALID"),
        );
        return None;
    }
    let id = match text(line, "id") {
        Some(id)
            if existing
                .iter()
                .any(|known| known.get("id").and_then(Value::as_str) == Some(id)) =>
        {
            id.to_string()
        }
        Some(_) => {
            errors.push(
                UserError::new(&model::path(path, &[kind, "id"]), "Line does not exist")
                    .code("INVALID"),
            );
            return None;
        }
        None => model::uuid_gid(state, "SubscriptionLine"),
    };
    let fields = if kind == "customLine" {
        value::object(json!({
            "productId": null,
            "variantId": null,
            "title": line.get("title"),
            "variantTitle": line.get("variantTitle"),
            "sku": line.get("sku"),
            "requiresShipping": line.get("requiresShipping").cloned().unwrap_or(json!(true)),
            "taxable": line.get("taxable").cloned().unwrap_or(json!(true)),
            "variantImage": null,
            "weight": line.get("weight"),
            "lsfPrice": crate::domains::orders::model::cents(line.get("price")),
        }))
    } else {
        let variant = text(line, "productVariantId").unwrap_or_default();
        match model::variant_fields(state, variant) {
            Some(fields) => fields,
            None => {
                diagnostics.push(diagnostic(
                    "MERCHANDISE_NOT_FOUND",
                    &model::path(path, &[kind, "productVariantId"]),
                ));
                return None;
            }
        }
    };
    let price = match line.get("priceOverride").filter(|price| !price.is_null()) {
        Some(price) => crate::domains::orders::model::cents(Some(price)),
        None => fields.get("lsfPrice").and_then(Value::as_i64).unwrap_or(0),
    };
    if !value::money_in_bounds(price.saturating_mul(quantity)) {
        errors.push(
            UserError::new(&model::path(path, &[kind]), value::money_too_large("Price"))
                .code("INVALID"),
        );
        return None;
    }
    let made = model::line_record(state, id.clone(), fields, quantity, price, line, currency);
    for discount in objects(line.get("discounts")) {
        if let Some(manual) = calculated_discount(
            state,
            &discount,
            "LINE_ITEM",
            currency,
            (false, vec![id.clone()]),
        ) {
            discounts.push(manual);
        }
    }
    Some(made)
}

/// A manual discount of the calculation API (`orderDiscount`, `deliveryDiscount`, a line's):
/// its `value` (`@oneOf` `percentage`, `fixedAmount`), its `recurringCycleLimit` (0: no
/// limit), its `id` when it restates one.
fn calculated_discount(
    state: &mut ShopState,
    input: &Map<String, Value>,
    target: &str,
    currency: &str,
    entitled: (bool, Vec<String>),
) -> Option<Value> {
    let given = object(input.get("value"));
    let value = if let Some(percentage) = given.get("percentage").and_then(Value::as_i64) {
        json!({"__typename": "SubscriptionDiscountPercentageValue", "percentage": percentage.clamp(0, 100)})
    } else {
        let fixed = object(given.get("fixedAmount"));
        json!({
            "__typename": "SubscriptionDiscountFixedAmountValue",
            "amount": model::money(crate::domains::orders::model::cents(fixed.get("amount")), currency),
            "appliesOnEachItem": fixed.get("appliesOnEachItem").and_then(Value::as_bool).unwrap_or(false),
        })
    };
    let id = text(input, "id")
        .map(str::to_string)
        .unwrap_or_else(|| model::uuid_gid(state, "SubscriptionManualDiscount"));
    let limit = input
        .get("recurringCycleLimit")
        .and_then(Value::as_i64)
        .filter(|limit| *limit > 0);
    Some(manual_discount(
        id,
        text(input, "title"),
        target,
        value,
        limit,
        "MANUAL",
        entitled,
    ))
}

/// Calculates a contract-like record from a calculation input over `base` (the contract, or an
/// empty one for a create). Answers the record, the diagnostics (failing), the warnings, the
/// delivery options when the input only fetches them; or user errors.
#[allow(clippy::type_complexity)]
fn calculate(
    state: &mut ShopState,
    mut record: Record,
    input: &Map<String, Value>,
    path: &[&str],
    policies: bool,
) -> Result<(Record, Vec<Value>, Vec<Value>, Option<Vec<Value>>), Vec<UserError>> {
    let currency = text(&record, "currencyCode").unwrap_or("USD").to_string();
    let mut errors = Vec::new();
    let mut diagnostics = Vec::new();
    let mut warnings = Vec::new();
    if !model::currency_enabled(state, &currency) {
        diagnostics.push(diagnostic(
            "CURRENCY_NOT_SUPPORTED",
            &model::path(path, &["currencyCode"]),
        ));
    }
    if policies
        && (input.contains_key("billingPolicy")
            || input.contains_key("deliveryPolicy")
            || input.contains_key("appManagedBillingConfig"))
    {
        let billing = object(input.get("billingPolicy"));
        let delivery = object(input.get("deliveryPolicy"));
        let mut config = object(input.get("appManagedBillingConfig"));
        if !input.contains_key("appManagedBillingConfig") {
            for field in ["minCycles", "maxCycles"] {
                if let Some(value) = record.pointer(&format!("/billingPolicy/{field}")) {
                    config.insert(field.into(), value.clone());
                }
            }
        }
        let (billing_policy, delivery_policy) = if billing.is_empty() {
            let mut kept_billing = record.get("billingPolicy").cloned().unwrap_or(Value::Null);
            for field in ["minCycles", "maxCycles"] {
                if let Some(value) = config.get(field) {
                    kept_billing[field] = value.clone();
                }
            }
            let kept_delivery = if delivery.is_empty() {
                record.get("deliveryPolicy").cloned().unwrap_or(Value::Null)
            } else {
                let cadence = kept_billing
                    .get("cadence")
                    .cloned()
                    .unwrap_or(json!({"count": 1, "unit": "MONTH"}));
                model::calculated_policies(
                    &value::object(json!({"cadence": cadence})),
                    &delivery,
                    &config,
                )
                .1
            };
            (kept_billing, kept_delivery)
        } else {
            model::calculated_policies(&billing, &delivery, &config)
        };
        model::check_policies(
            Some(&billing_policy),
            Some(&delivery_policy),
            path,
            &mut errors,
        );
        record.insert("billingPolicy".into(), billing_policy);
        record.insert("deliveryPolicy".into(), delivery_policy);
    }
    let mut options = None;
    if let Some(method) = input.get("deliveryMethod").filter(|value| !value.is_null()) {
        match model::calculated_delivery_method(
            state,
            &object(Some(method)),
            &model::path(path, &["deliveryMethod"]),
        ) {
            Ok(Some((method, price))) => {
                record.insert("deliveryMethod".into(), method);
                record.insert("deliveryPrice".into(), model::money(price, &currency));
            }
            Ok(None) => {
                record.insert("deliveryMethod".into(), Value::Null);
                options = Some(rate_options(state, &currency, false, true));
            }
            Err(more) => errors.extend(more),
        }
    }
    if let Some(method) = input.get("paymentMethod").filter(|value| !value.is_null()) {
        match method
            .get("customerPaymentMethod")
            .and_then(|method| method.get("id"))
            .and_then(Value::as_str)
        {
            Some(id) => {
                let customer = text(&record, "customer").map(str::to_string);
                if model::check_payment_method(state, id, customer.as_deref(), &[]).is_some() {
                    diagnostics.push(diagnostic(
                        "PAYMENT_METHOD_NOT_FOUND",
                        &model::path(path, &["paymentMethod"]),
                    ));
                } else {
                    record.insert("customerPaymentMethod".into(), json!(id));
                }
            }
            None => {
                record.insert("customerPaymentMethod".into(), Value::Null);
            }
        }
    }
    let mut line_discounts = Vec::new();
    if let Some(given) = input.get("lines").filter(|value| !value.is_null()) {
        let existing = lines(&record);
        let mut made = Vec::new();
        for (index, line) in objects(Some(given)).iter().enumerate() {
            let index = index.to_string();
            if let Some(line) = calculated_line(
                state,
                line,
                &currency,
                &existing,
                &model::path(path, &["lines", &index]),
                &mut diagnostics,
                &mut errors,
                &mut line_discounts,
            ) {
                made.push(line);
            }
        }
        record.insert("lines".into(), Value::Array(made));
        // A restated line keeps none of its stored discounts: the lines' discounts are given
        // again with them.
        let kept: Vec<Value> = model::discounts(&record)
            .into_iter()
            .filter(|discount| {
                discount
                    .pointer("/entitledLines/all")
                    .and_then(Value::as_bool)
                    != Some(false)
            })
            .collect();
        record.insert("discounts".into(), Value::Array(kept));
    }
    if let Some(manual) = input
        .get("manualDiscounts")
        .filter(|value| !value.is_null())
    {
        let mut discounts: Vec<Value> = model::discounts(&record)
            .into_iter()
            .filter(|discount| {
                discount.get("type").and_then(Value::as_str) == Some("CODE_DISCOUNT")
            })
            .collect();
        for discount in objects(Some(manual)) {
            if let Some(order) = discount.get("orderDiscount").and_then(Value::as_object)
                && let Some(made) =
                    calculated_discount(state, order, "LINE_ITEM", &currency, (true, Vec::new()))
            {
                discounts.push(made);
            }
            if let Some(delivery) = discount.get("deliveryDiscount").and_then(Value::as_object)
                && let Some(made) = calculated_discount(
                    state,
                    delivery,
                    "SHIPPING_LINE",
                    &currency,
                    (true, Vec::new()),
                )
            {
                discounts.push(made);
            }
        }
        record.insert("discounts".into(), Value::Array(discounts));
    }
    if !line_discounts.is_empty() {
        let mut discounts = model::discounts(&record);
        discounts.extend(line_discounts);
        record.insert("discounts".into(), Value::Array(discounts));
    }
    for (index, code) in objects(input.get("discountCodes")).iter().enumerate() {
        let redeem = text(code, "redeemCode").unwrap_or_default();
        let applied = applied_code(state, redeem, &currency);
        if applied
            .get("rejectionReason")
            .is_some_and(|reason| !reason.is_null())
        {
            let index = index.to_string();
            warnings.push(diagnostic(
                "DISCOUNT_CODE_NOT_APPLIED",
                &model::path(path, &["discountCodes", &index]),
            ));
        } else {
            let mut discounts = model::discounts(&record);
            let mut manual = applied.get("lsfManual").cloned().unwrap_or(Value::Null);
            manual["id"] = applied.get("id").cloned().unwrap_or(Value::Null);
            discounts.push(manual);
            record.insert("discounts".into(), Value::Array(discounts));
        }
    }
    if let Some(attributes) = input
        .get("customAttributes")
        .filter(|value| !value.is_null())
    {
        record.insert(
            "customAttributes".into(),
            crate::domains::orders::model::attributes(Some(attributes)),
        );
    }
    model::check_note(input, path, &mut errors);
    if let Some(note) = input.get("note") {
        record.insert("note".into(), note.clone());
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    if options.is_none() {
        let ships = model::requires_shipping(&record);
        let has_method = record
            .get("deliveryMethod")
            .is_some_and(|method| !method.is_null());
        if ships && !has_method {
            diagnostics.push(diagnostic(
                "DELIVERY_METHOD_REQUIRED",
                &model::path(path, &["deliveryMethod"]),
            ));
        } else if !ships && has_method {
            warnings.push(diagnostic(
                "DELIVERY_METHOD_UNUSED",
                &model::path(path, &["deliveryMethod"]),
            ));
        }
    }
    if lines(&record).is_empty() && diagnostics.is_empty() {
        diagnostics.push(diagnostic("INVALID", &model::path(path, &["lines"])));
    }
    refresh(state, &mut record);
    Ok((record, diagnostics, warnings, options))
}

/// Keeps a calculation, its outcome, and answers it pending.
fn keep(
    state: &mut ShopState,
    target: Target,
    record: Record,
    diagnostics: Vec<Value>,
    warnings: Vec<Value>,
    options: Option<Vec<Value>>,
    with_customizations: bool,
) -> Value {
    let number = model::number(&state.new_gid("SubscriptionContractCalculation"));
    let id = format!("gid://shopify/SubscriptionContractCalculation/{number}");
    let (kind, contract, index) = match &target {
        Target::Create => ("create", None, None),
        Target::Update(contract) => ("update", Some(contract.clone()), None),
        Target::Cycle(contract, index) => ("cycle", Some(contract.clone()), Some(*index)),
    };
    let revision = contract
        .as_deref()
        .map(|contract| model::revision(state, contract));
    let mut calculation = Record::new();
    calculation.insert("id".into(), json!(id));
    if diagnostics.is_empty() {
        let mut calculated = record.clone();
        calculated.insert("__typename".into(), json!("SubscriptionCalculatedContract"));
        let totals = options
            .is_none()
            .then(|| model::projected_totals(state, &calculated));
        calculation.insert("__typename".into(), json!(CALCULATION));
        calculation.insert("calculatedContract".into(), Value::Object(calculated));
        calculation.insert(
            "deliveryOptions".into(),
            json!(options.clone().unwrap_or_default()),
        );
        calculation.insert("projectedOrderTotals".into(), json!(totals));
        calculation.insert("warnings".into(), Value::Array(warnings));
        calculation.insert(
            "withMerchandiseCustomizations".into(),
            json!(with_customizations),
        );
    } else {
        calculation.insert(
            "__typename".into(),
            json!("SubscriptionContractCalculationFailure"),
        );
        calculation.insert("errors".into(), Value::Array(diagnostics));
    }
    calculation.insert(
        "lsfTarget".into(),
        json!({"kind": kind, "contract": contract, "index": index}),
    );
    calculation.insert("lsfRevision".into(), json!(revision));
    calculation.insert("lsfDiscovery".into(), json!(options.is_some()));
    calculation.insert("lsfCommitted".into(), json!(false));
    state.insert_as(id.clone(), calculation);
    state.emit(crate::domains::app::CHANGED, &id);
    json!({"__typename": "SubscriptionContractCalculationPending", "id": id})
}

/// The user errors of a calculate mutation, in its error type (whose codes are `INVALID`, and
/// for an update `CONTRACT_TERMINATED`, `CONTRACT_HAS_FUTURE_EDITS`).
fn invalid(errors: Vec<UserError>) -> Vec<UserError> {
    errors
        .into_iter()
        .map(|error| UserError {
            code: Some("INVALID".into()),
            ..error
        })
        .collect()
}

/// `subscriptionContractCreateCalculate`.
fn create_calculate(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"subscriptionContractCalculation": null});
    let input = object(ctx.arg("contractCreateInput"));
    let customer = text(&input, "customerId").unwrap_or_default().to_string();
    if mutation.state().type_of(&customer) != Some("Customer") {
        return Ok(payload(
            empty,
            &[UserError::new(
                &["contractCreateInput", "customerId"],
                model::described("SubscriptionDraftErrorCode", "CUSTOMER_DOES_NOT_EXIST"),
            )
            .code("INVALID")],
        ));
    }
    let currency = text(&input, "currencyCode").unwrap_or("USD").to_string();
    let base = value::object(json!({
        "status": "ACTIVE",
        "customer": customer,
        "customerPaymentMethod": null,
        "currencyCode": currency,
        "billingPolicy": null,
        "deliveryPolicy": null,
        "deliveryMethod": null,
        "deliveryPrice": model::money(0, &currency),
        "lines": [],
        "discounts": [],
        "customAttributes": [],
        "note": null,
    }));
    let with_customizations = input
        .get("withMerchandiseCustomizations")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state = mutation.state_mut();
    match calculate(state, base, &input, &["contractCreateInput"], true) {
        Ok((record, diagnostics, warnings, options)) => {
            let pending = keep(
                state,
                Target::Create,
                record,
                diagnostics,
                warnings,
                options,
                with_customizations,
            );
            Ok(payload(
                json!({"subscriptionContractCalculation": pending}),
                &[],
            ))
        }
        Err(errors) => Ok(payload(empty, &invalid(errors))),
    }
}

/// `subscriptionContractUpdateCalculate`.
fn update_calculate(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"subscriptionContractCalculation": null});
    let id = ctx.arg_str("contractId").unwrap_or_default().to_string();
    let input = object(ctx.arg("contractUpdateInput"));
    let state = mutation.state();
    let error = |code: &str| {
        UserError::new(
            &["contractId"],
            model::described("SubscriptionContractUpdateCalculateUserErrorCode", code),
        )
        .code(code)
    };
    let Some(contract) = contract_arg(state, &id) else {
        return Ok(payload(
            empty,
            &[UserError::new(&["contractId"], "Contract does not exist").code("INVALID")],
        ));
    };
    if is_terminated(&contract) {
        return Ok(payload(empty, &[error("CONTRACT_TERMINATED")]));
    }
    if cycles::has_future_contract_edits(state, &contract) {
        return Ok(payload(empty, &[error("CONTRACT_HAS_FUTURE_EDITS")]));
    }
    let with_customizations = input
        .get("withMerchandiseCustomizations")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state = mutation.state_mut();
    match calculate(state, contract, &input, &["contractUpdateInput"], true) {
        Ok((record, diagnostics, warnings, options)) => {
            let pending = keep(
                state,
                Target::Update(id),
                record,
                diagnostics,
                warnings,
                options,
                with_customizations,
            );
            Ok(payload(
                json!({"subscriptionContractCalculation": pending}),
                &[],
            ))
        }
        Err(errors) => Ok(payload(empty, &invalid(errors))),
    }
}

/// `subscriptionBillingCycleContractEditCalculate`: the contract of one cycle.
fn cycle_edit_calculate(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"subscriptionContractCalculation": null});
    let id = ctx.arg_str("contractId").unwrap_or_default().to_string();
    let input = object(ctx.arg("billingCycleEditInput"));
    let state = mutation.state();
    let Some(contract) = contract_arg(state, &id) else {
        return Ok(payload(
            empty,
            &[UserError::new(&["contractId"], "Contract does not exist").code("INVALID")],
        ));
    };
    if is_terminated(&contract) {
        return Ok(payload(
            empty,
            &[UserError::new(
                &["contractId"],
                model::described(
                    "SubscriptionContractUpdateCalculateUserErrorCode",
                    "CONTRACT_TERMINATED",
                ),
            )
            .code("INVALID")],
        ));
    }
    let cycle = match cycles::select(state, &contract, &object(ctx.arg("billingCycleSelector"))) {
        Ok(cycle) => cycle,
        Err(miss) => {
            return Ok(payload(
                empty,
                &[UserError::new(
                    &["billingCycleSelector"],
                    model::described("SubscriptionDraftErrorCode", miss.draft_code()),
                )
                .code("INVALID")],
            ));
        }
    };
    if cycles::is_billed(state, &contract, cycle.index) {
        return Ok(payload(
            empty,
            &[UserError::new(
                &["billingCycleSelector"],
                "Billing cycle has already been billed",
            )
            .code("INVALID")],
        ));
    }
    let base = super::drafts::cycle_contract(&contract, cycle.index);
    let mut base = base;
    for field in [
        "customer",
        "currencyCode",
        "billingPolicy",
        "deliveryPolicy",
    ] {
        base.insert(
            field.into(),
            contract.get(field).cloned().unwrap_or(Value::Null),
        );
    }
    let with_customizations = input
        .get("withMerchandiseCustomizations")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state = mutation.state_mut();
    match calculate(state, base, &input, &["billingCycleEditInput"], false) {
        Ok((record, diagnostics, warnings, options)) => {
            let pending = keep(
                state,
                Target::Cycle(id, cycle.index),
                record,
                diagnostics,
                warnings,
                options,
                with_customizations,
            );
            Ok(payload(
                json!({"subscriptionContractCalculation": pending}),
                &[],
            ))
        }
        Err(errors) => Ok(payload(empty, &invalid(errors))),
    }
}

/// A `SubscriptionContractCalculationCommitUserError`.
fn commit_error(code: &str) -> UserError {
    UserError::new(
        &["id"],
        model::described("SubscriptionContractCalculationCommitUserErrorCode", code),
    )
    .code(code)
}

/// `subscriptionContractCalculationCommit`.
fn commit(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"contract": null});
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(calculation) = state.get(&id).filter(|record| {
        matches!(
            text(record, "__typename"),
            Some(CALCULATION | "SubscriptionContractCalculationFailure")
        )
    }) else {
        return Ok(payload(empty, &[commit_error("CALCULATION_NOT_FOUND")]));
    };
    let calculation = calculation.clone();
    if text(&calculation, "__typename") != Some(CALCULATION) {
        return Ok(payload(empty, &[commit_error("NOT_READY_TO_COMMIT")]));
    }
    if calculation.get("lsfCommitted").and_then(Value::as_bool) == Some(true) {
        // lsf's wording: a calculation commits once.
        return Ok(payload(
            empty,
            &[
                UserError::new(&["id"], "The calculation has already been committed")
                    .code("INVALID"),
            ],
        ));
    }
    if calculation.get("lsfDiscovery").and_then(Value::as_bool) == Some(true) {
        return Ok(payload(
            empty,
            &[commit_error("DISCOVERY_CALCULATION_NOT_COMMITTABLE")],
        ));
    }
    let target = object(calculation.get("lsfTarget"));
    let contract = text(&target, "contract").map(str::to_string);
    if let Some(contract) = &contract {
        let Some(record) = contract_arg(state, contract) else {
            return Ok(payload(empty, &[commit_error("CALCULATION_NOT_FOUND")]));
        };
        if is_terminated(&record) {
            return Ok(payload(empty, &[commit_error("CONTRACT_TERMINATED")]));
        }
        if text(&calculation, "lsfRevision") != Some(model::revision(state, contract).as_str()) {
            return Ok(payload(empty, &[commit_error("STALE_CONTRACT")]));
        }
    }
    let mut calculated = object(calculation.get("calculatedContract"));
    calculated.remove("__typename");
    let state = mutation.state_mut();
    let answer = match (text(&target, "kind"), contract) {
        (Some("cycle"), Some(contract)) => {
            let index = target.get("index").and_then(Value::as_i64).unwrap_or(0);
            commit_cycle_edit(state, &contract, index, &calculated, &[])
        }
        (Some("update"), Some(contract)) => {
            commit_update(state, &contract, &calculated);
            json!(contract)
        }
        _ => {
            let made = commit_new(state, &calculated, None);
            // The first billing date of a contract of the calculation API is its first cycle's
            // (`appManagedBillingConfig` has no next billing date).
            if let Some(record) = state.get(&made).cloned()
                && let Some(cycle) = cycles::cycle(state, &record, 1)
            {
                let next = value::date_time(cycles::expected_date(&record, &cycle));
                if let Some(record) = state.get_mut(&made) {
                    record.insert("nextBillingDate".into(), json!(next));
                }
            }
            json!(made)
        }
    };
    if let Some(record) = state.get_mut(&id) {
        record.insert("lsfCommitted".into(), json!(true));
    }
    Ok(payload(json!({"contract": answer}), &[]))
}
