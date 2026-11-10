//! Selling plan groups: `sellingPlanGroupCreate`, `sellingPlanGroupUpdate`,
//! `sellingPlanGroupDelete`, `sellingPlanGroupAddProducts`, `sellingPlanGroupRemoveProducts`,
//! `sellingPlanGroupAddProductVariants`, `sellingPlanGroupRemoveProductVariants`,
//! `productJoinSellingPlanGroups`, `productLeaveSellingPlanGroups`, their variant forms, and the
//! `appliesTo…` fields of a group.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A group has a `name`, a `merchantCode`, `options` (the labels of its plans' options, at
//!   most 3), and its selling plans: each with a `name`, one option value per group option, a
//!   billing and a delivery policy of the same kind (recurring or fixed), at most 2 pricing
//!   policies (a fixed one first), `category`, `inventoryPolicy`, `metafields`. The checks are
//!   those the codes of `SellingPlanGroupUserErrorCode` name, worded as their descriptions
//!   ("Billing and delivery policy types must be the same.", "Cannot have multiple selling plans
//!   with the same name.", …).
//! - `resources` (`productIds`, `productVariantIds`) and the add/remove mutations link the group
//!   with products and variants, each side listing the other (`Product.sellingPlanGroups`,
//!   `ProductVariant.sellingPlanGroups`).
//! - `productJoinSellingPlanGroups`, `productLeaveSellingPlanGroups` and their variant forms do
//!   the same from the product's (or the variant's) side.
//! - Events: `selling_plan_groups/create`, `selling_plan_groups/update` (also when its products
//!   change), `selling_plan_groups/delete`.
//!
//! The store sync writes a group's name, app id, options, products and plans back to the store
//! data, which keeps a plan's pricing, whether it recurs and its checkout charge (not its
//! intervals or anchors).

use serde_json::{Map, Value, json};

use super::gids;
use super::records::{self, field, objects, text};
use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, coded, payload};
use crate::admin::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("sellingPlanGroupCreate", group_create)
        .mutation("sellingPlanGroupUpdate", group_update)
        .mutation("sellingPlanGroupDelete", group_delete)
        .mutation("sellingPlanGroupAddProducts", add_products)
        .mutation("sellingPlanGroupRemoveProducts", remove_products)
        .mutation("sellingPlanGroupAddProductVariants", add_variants)
        .mutation("sellingPlanGroupRemoveProductVariants", remove_variants)
        .mutation("productJoinSellingPlanGroups", product_join)
        .mutation("productLeaveSellingPlanGroups", product_leave)
        .mutation("productVariantJoinSellingPlanGroups", variant_join)
        .mutation("productVariantLeaveSellingPlanGroups", variant_leave)
        .field("SellingPlanGroup", "appliesToProduct", applies_to_product)
        .field(
            "SellingPlanGroup",
            "appliesToProductVariant",
            applies_to_variant,
        )
        .field(
            "SellingPlanGroup",
            "appliesToProductVariants",
            applies_to_variants,
        )
        .field("SellingPlanGroup", "productVariants", group_variants)
        .field(
            "SellingPlanGroup",
            "productVariantsCount",
            group_variants_count,
        );
}

/// The most selling plans a group has.
const PLAN_LIMIT: usize = 31;

/// `appliesToProduct(productId:)`: the group lists the product.
fn applies_to_product(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("productId").unwrap_or_default();
    let applies = r
        .items_of(object, "products")
        .iter()
        .any(|item| item.as_str() == Some(id));
    Ok(Output::value(applies))
}

/// `appliesToProductVariant(productVariantId:)`: the group lists the variant.
fn applies_to_variant(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("productVariantId").unwrap_or_default();
    let applies = r
        .items_of(object, "productVariants")
        .iter()
        .any(|item| item.as_str() == Some(id));
    Ok(Output::value(applies))
}

/// The variants of a group, of one product when `productId` is given.
fn variants_of(r: &ShopResolver<'_>, ctx: &FieldContext<'_>, object: &Obj) -> Vec<Value> {
    let product = ctx.arg_str("productId");
    r.items_of(object, "productVariants")
        .into_iter()
        .filter(|item| {
            product.is_none_or(|product| {
                r.item_map(item)
                    .and_then(|variant| field(variant, "product"))
                    == Some(product)
            })
        })
        .collect()
}

/// `appliesToProductVariants(productId:)`: the group lists a variant of the product.
fn applies_to_variants(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(Output::value(!variants_of(r, ctx, object).is_empty()))
}

/// `productVariants(productId:)`.
fn group_variants(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, variants_of(r, ctx, object))
}

/// `productVariantsCount(productId:)`.
fn group_variants_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(r.count(ctx, variants_of(r, ctx, object).len()))
}

/// A pricing policy's value: a percentage, or an amount in the shop's currency.
fn adjustment_value(kind: &str, value: &Map<String, Value>, currency: &str) -> Value {
    if kind == "PERCENTAGE" {
        return json!({
            "__typename": "SellingPlanPricingPolicyPercentageValue",
            "percentage": value.get("percentage").and_then(Value::as_f64).unwrap_or(0.0),
        });
    }
    let cents = value
        .get("fixedValue")
        .and_then(super::variants::money_cents)
        .unwrap_or(0);
    let mut money = value::money_v2(cents, currency);
    money["__typename"] = json!("MoneyV2");
    money
}

/// The anchors of a policy input.
fn anchors(input: &Map<String, Value>) -> Value {
    Value::Array(
        objects(input, "anchors")
            .iter()
            .map(|anchor| {
                json!({
                    "type": anchor.get("type"),
                    "day": anchor.get("day"),
                    "month": anchor.get("month"),
                    "cutoffDay": anchor.get("cutoffDay"),
                })
            })
            .collect(),
    )
}

/// Checks a `SellingPlanInput` (`path` its place in the input), against the group's options.
fn check_plan(
    plan: &Map<String, Value>,
    options: usize,
    creating: bool,
    path: &[String],
    errors: &mut Vec<UserError>,
) {
    let at =
        |name: &str| -> Vec<String> { path.iter().cloned().chain([name.to_string()]).collect() };
    let push = |errors: &mut Vec<UserError>, at: Vec<String>, message: &str, code: &str| {
        errors.push(UserError {
            field: Some(at),
            message: message.to_string(),
            code: Some(code.to_string()),
        })
    };
    if creating && text(plan, "name").is_none_or(|name| name.trim().is_empty()) {
        push(errors, at("name"), "Name can't be blank", "BLANK");
    }
    if let Some(given) = plan.get("options").and_then(Value::as_array) {
        if given.len() > options {
            let code = if given.len() == 2 {
                "SELLING_PLAN_MISSING_OPTION2_LABEL_ON_PARENT_GROUP"
            } else {
                "SELLING_PLAN_MISSING_OPTION3_LABEL_ON_PARENT_GROUP"
            };
            let message = if given.len() == 2 {
                "Cannot define option2 on this selling plan as there's no label on the parent selling plan group."
            } else {
                "Cannot define option3 on this selling plan as there's no label on the parent selling plan group."
            };
            push(errors, at("options"), message, code);
        } else if given.len() < options {
            let (message, code) = if given.len() < 2 && options >= 2 {
                (
                    "Selling plan's option2 is required because option2 exists.",
                    "SELLING_PLAN_OPTION2_REQUIRED_AS_DEFINED_ON_PARENT_GROUP",
                )
            } else {
                (
                    "Selling plan's option3 is required because option3 exists.",
                    "SELLING_PLAN_OPTION3_REQUIRED_AS_DEFINED_ON_PARENT_GROUP",
                )
            };
            push(errors, at("options"), message, code);
        }
    } else if creating && options > 0 {
        push(errors, at("options"), "Options can't be blank", "BLANK");
    }
    let kind = |name: &str| -> Option<&str> {
        let policy = plan.get(name)?.as_object()?;
        match (policy.get("recurring"), policy.get("fixed")) {
            (Some(_), Some(_)) => Some("both"),
            (Some(_), None) => Some("recurring"),
            (None, Some(_)) => Some("fixed"),
            (None, None) => None,
        }
    };
    let (billing, delivery) = (kind("billingPolicy"), kind("deliveryPolicy"));
    if creating && billing.is_none() {
        push(
            errors,
            at("billingPolicy"),
            "Missing billing policy.",
            "SELLING_PLAN_BILLING_POLICY_MISSING",
        );
    }
    if creating && delivery.is_none() {
        push(
            errors,
            at("deliveryPolicy"),
            "Missing delivery policy.",
            "SELLING_PLAN_DELIVERY_POLICY_MISSING",
        );
    }
    if billing == Some("both") {
        push(
            errors,
            at("billingPolicy"),
            "A selling plan can't have both fixed and recurring billing policies.",
            "ONLY_ONE_OF_FIXED_OR_RECURRING_BILLING",
        );
    }
    if delivery == Some("both") {
        push(
            errors,
            at("deliveryPolicy"),
            "A selling plan can't have both fixed and recurring delivery policies.",
            "ONLY_ONE_OF_FIXED_OR_RECURRING_DELIVERY",
        );
    }
    if let (Some(billing), Some(delivery)) = (billing, delivery)
        && billing != delivery
        && billing != "both"
        && delivery != "both"
    {
        push(
            errors,
            path.to_vec(),
            "Billing and delivery policy types must be the same.",
            "BILLING_AND_DELIVERY_POLICY_TYPES_MUST_BE_THE_SAME",
        );
    }
    let pricing = objects(plan, "pricingPolicies");
    if pricing.len() > 2 {
        push(
            errors,
            at("pricingPolicies"),
            "Selling plans can't have more than 2 pricing policies.",
            "SELLING_PLAN_PRICING_POLICIES_LIMIT",
        );
    }
    if !pricing.is_empty() && !pricing.iter().any(|policy| policy.contains_key("fixed")) {
        push(
            errors,
            at("pricingPolicies"),
            "Selling plan's pricing policies must contain one fixed pricing policy.",
            "SELLING_PLAN_PRICING_POLICIES_MUST_CONTAIN_A_FIXED_PRICING_POLICY",
        );
    }
    if billing == Some("fixed") && pricing.len() > 1 {
        push(
            errors,
            at("pricingPolicies"),
            "A fixed selling plan can have at most one pricing policy.",
            "SELLING_PLAN_FIXED_PRICING_POLICIES_LIMIT",
        );
    }
    for (index, policy) in pricing.iter().enumerate() {
        let inner = policy
            .get("fixed")
            .or_else(|| policy.get("recurring"))
            .and_then(Value::as_object);
        let Some(inner) = inner else { continue };
        let kind = text(inner, "adjustmentType").unwrap_or("PERCENTAGE");
        let value = inner.get("adjustmentValue").and_then(Value::as_object);
        let has_percentage =
            value.is_some_and(|value| value.get("percentage").is_some_and(|v| !v.is_null()));
        let has_amount =
            value.is_some_and(|value| value.get("fixedValue").is_some_and(|v| !v.is_null()));
        if (kind == "PERCENTAGE" && has_amount) || (kind != "PERCENTAGE" && has_percentage) {
            push(
                errors,
                path.iter()
                    .cloned()
                    .chain(["pricingPolicies".to_string(), index.to_string()])
                    .collect(),
                "Pricing policy's adjustment value and adjustment type must match.",
                "PRICING_POLICY_ADJUSTMENT_VALUE_AND_TYPE_MUST_MATCH",
            );
        }
    }
}

/// Writes a selling plan's fields from its input over `record` (a new plan's or the current
/// one).
fn plan_record(
    state: &ShopState,
    record: &mut Map<String, Value>,
    plan: &Map<String, Value>,
    now: &str,
) {
    let currency = state.currency();
    for name in ["name", "description", "category"] {
        if let Some(given) = plan.get(name) {
            record.insert(name.into(), given.clone());
        }
    }
    if let Some(options) = plan.get("options") {
        record.insert("options".into(), options.clone());
    }
    if let Some(position) = plan.get("position") {
        record.insert("position".into(), position.clone());
    }
    if let Some(reserve) = plan
        .get("inventoryPolicy")
        .and_then(|policy| policy.get("reserve"))
    {
        record.insert("inventoryPolicy".into(), json!({"reserve": reserve}));
    }
    if let Some(policy) = plan.get("billingPolicy").and_then(Value::as_object) {
        if let Some(recurring) = policy.get("recurring").and_then(Value::as_object) {
            record.insert(
                "billingPolicy".into(),
                json!({
                    "__typename": "SellingPlanRecurringBillingPolicy",
                    "interval": recurring.get("interval").cloned().unwrap_or(json!("MONTH")),
                    "intervalCount": recurring.get("intervalCount").cloned().unwrap_or(json!(1)),
                    "anchors": anchors(recurring),
                    "minCycles": recurring.get("minCycles"),
                    "maxCycles": recurring.get("maxCycles"),
                    "createdAt": now,
                }),
            );
        } else if let Some(fixed) = policy.get("fixed").and_then(Value::as_object) {
            let charge = fixed.get("checkoutCharge").and_then(Value::as_object);
            let charge_type = charge
                .and_then(|charge| text(charge, "type"))
                .unwrap_or("PERCENTAGE");
            let charge_value = charge
                .and_then(|charge| charge.get("value"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let value = match charge_type {
                "PERCENTAGE" => json!({
                    "__typename": "SellingPlanCheckoutChargePercentageValue",
                    "percentage": charge_value.get("percentage").and_then(Value::as_f64).unwrap_or(100.0),
                }),
                _ => adjustment_value("PRICE", &charge_value, &currency),
            };
            record.insert(
                "billingPolicy".into(),
                json!({
                    "__typename": "SellingPlanFixedBillingPolicy",
                    "checkoutCharge": {"type": charge_type, "value": value},
                    "remainingBalanceChargeTrigger": fixed.get("remainingBalanceChargeTrigger").cloned().unwrap_or(json!("NO_REMAINING_BALANCE")),
                    "remainingBalanceChargeExactTime": fixed.get("remainingBalanceChargeExactTime"),
                    "remainingBalanceChargeTimeAfterCheckout": fixed.get("remainingBalanceChargeTimeAfterCheckout"),
                }),
            );
        }
    }
    if let Some(policy) = plan.get("deliveryPolicy").and_then(Value::as_object) {
        if let Some(recurring) = policy.get("recurring").and_then(Value::as_object) {
            record.insert(
                "deliveryPolicy".into(),
                json!({
                    "__typename": "SellingPlanRecurringDeliveryPolicy",
                    "interval": recurring.get("interval").cloned().unwrap_or(json!("MONTH")),
                    "intervalCount": recurring.get("intervalCount").cloned().unwrap_or(json!(1)),
                    "anchors": anchors(recurring),
                    "cutoff": recurring.get("cutoff"),
                    "intent": recurring.get("intent").cloned().unwrap_or(json!("FULFILLMENT_BEGIN")),
                    "preAnchorBehavior": recurring.get("preAnchorBehavior").cloned().unwrap_or(json!("ASAP")),
                    "createdAt": now,
                }),
            );
        } else if let Some(fixed) = policy.get("fixed").and_then(Value::as_object) {
            record.insert(
                "deliveryPolicy".into(),
                json!({
                    "__typename": "SellingPlanFixedDeliveryPolicy",
                    "anchors": anchors(fixed),
                    "cutoff": fixed.get("cutoff"),
                    "fulfillmentExactTime": fixed.get("fulfillmentExactTime"),
                    "fulfillmentTrigger": fixed.get("fulfillmentTrigger").cloned().unwrap_or(json!("ASAP")),
                    "intent": fixed.get("intent").cloned().unwrap_or(json!("FULFILLMENT_BEGIN")),
                    "preAnchorBehavior": fixed.get("preAnchorBehavior").cloned().unwrap_or(json!("ASAP")),
                }),
            );
        }
    }
    if plan.contains_key("pricingPolicies") {
        let policies: Vec<Value> = objects(plan, "pricingPolicies")
            .iter()
            .filter_map(|policy| {
                let (type_name, inner) = match (policy.get("fixed"), policy.get("recurring")) {
                    (Some(fixed), _) => ("SellingPlanFixedPricingPolicy", fixed.as_object()?),
                    (None, Some(recurring)) => {
                        ("SellingPlanRecurringPricingPolicy", recurring.as_object()?)
                    }
                    _ => return None,
                };
                let kind = text(inner, "adjustmentType").unwrap_or("PERCENTAGE");
                let value = inner
                    .get("adjustmentValue")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let mut policy = json!({
                    "__typename": type_name,
                    "adjustmentType": kind,
                    "adjustmentValue": adjustment_value(kind, &value, &currency),
                    "createdAt": now,
                });
                if let Some(after) = inner.get("afterCycle") {
                    policy["afterCycle"] = after.clone();
                }
                Some(policy)
            })
            .collect();
        record.insert("pricingPolicies".into(), json!(policies));
    }
}

/// Creates the plans of `sellingPlansToCreate`. Returns their GIDs.
fn create_plans(
    state: &mut ShopState,
    plans: &[&Map<String, Value>],
    first_position: usize,
) -> Vec<String> {
    let now = state.timestamp();
    let mut ids = Vec::new();
    for (index, plan) in plans.iter().enumerate() {
        let id = state.new_gid("SellingPlan");
        let mut record = value::object(json!({
            "__typename": "SellingPlan",
            "id": id,
            "name": "",
            "description": null,
            "options": [],
            "position": first_position + index,
            "category": null,
            "billingPolicy": null,
            "deliveryPolicy": null,
            "pricingPolicies": [],
            "createdAt": now,
            "metafields": [],
        }));
        plan_record(state, &mut record, plan, &now);
        state.insert(record);
        let metafields = objects(plan, "metafields");
        records::set_metafields(state, &id, "SELLING_PLAN", &metafields);
        ids.push(id);
    }
    ids
}

/// The summary Shopify writes of a group: "1 delivery frequency, 10% discount" (the form of the
/// webhook example's `summary`, "1 delivery frequency,  discount").
fn summary(state: &ShopState, plans: &[String]) -> String {
    let discounts: Vec<String> = plans
        .iter()
        .filter_map(|plan| state.get(plan))
        .filter_map(|plan| plan.get("pricingPolicies")?.as_array()?.first().cloned())
        .filter_map(|policy| {
            let value = policy.get("adjustmentValue")?;
            match value.get("percentage").and_then(Value::as_f64) {
                Some(percentage) => Some(format!("{}%", trim_float(percentage))),
                None => value
                    .get("amount")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }
        })
        .collect();
    let frequencies = plans.len();
    let noun = if frequencies == 1 {
        "frequency"
    } else {
        "frequencies"
    };
    let discount = match discounts.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        many => format!("{}-{}", many[0], many[many.len() - 1]),
    };
    format!("{frequencies} delivery {noun}, {discount} discount")
}

fn trim_float(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// The products and variants of `resources`, checked.
fn check_resources(
    state: &ShopState,
    products: &[String],
    variants: &[String],
    errors: &mut Vec<UserError>,
) {
    for (index, product) in products.iter().enumerate() {
        if !state.is_of(product, "Product") {
            errors.push(coded(
                &["resources", "productIds", &index.to_string()],
                "Product does not exist.",
                "PRODUCT_DOES_NOT_EXIST",
            ));
        }
    }
    for (index, variant) in variants.iter().enumerate() {
        if !state.is_of(variant, "ProductVariant") {
            errors.push(coded(
                &["resources", "productVariantIds", &index.to_string()],
                "Product variant does not exist.",
                "PRODUCT_VARIANT_DOES_NOT_EXIST",
            ));
        }
    }
}

/// Links (or unlinks) a group with products or variants, each listing the other. Whether
/// anything changed.
fn link(
    state: &mut ShopState,
    group: &str,
    list: &str,
    resources: &[String],
    member: bool,
) -> bool {
    let mut changed = false;
    for resource in resources {
        let listed = state
            .get(group)
            .is_some_and(|record| gids(record.get(list)).contains(resource));
        if listed != member {
            changed = true;
            if let Some(record) = state.get_mut(group) {
                let mut items = gids(record.get(list));
                if member {
                    items.push(resource.clone());
                } else {
                    items.retain(|known| known != resource);
                }
                record.insert(list.into(), json!(items));
            }
        }
        let has = state.get(resource).is_some_and(|record| {
            gids(record.get("sellingPlanGroups"))
                .iter()
                .any(|known| known == group)
        });
        if has != member
            && let Some(record) = state.get_mut(resource)
        {
            let mut groups = gids(record.get("sellingPlanGroups"));
            if member {
                groups.push(group.to_string());
            } else {
                groups.retain(|known| known != group);
            }
            record.insert("sellingPlanGroups".into(), json!(groups));
        }
    }
    changed
}

/// The `metafields` of a plan input (`path` its place), checked as `metafieldsSet` checks them
/// ([`records::check_metafields`]).
fn check_plan_metafields(
    state: &ShopState,
    plan_id: Option<&str>,
    plan: &Map<String, Value>,
    path: &[String],
    errors: &mut Vec<UserError>,
) {
    let mut at: Vec<&str> = path.iter().map(String::as_str).collect();
    at.push("metafields");
    errors.extend(records::check_metafields(
        state,
        plan_id,
        "SELLING_PLAN",
        &objects(plan, "metafields"),
        &at,
        Some("SellingPlanGroupUserErrorCode"),
    ));
}

/// The checks of a group's input: its name and options, its plans.
fn check_group(
    state: &ShopState,
    input: &Map<String, Value>,
    group: Option<&str>,
    errors: &mut Vec<UserError>,
) {
    let creating = group.is_none();
    if (creating || input.contains_key("name"))
        && text(input, "name").is_none_or(|name| name.trim().is_empty())
    {
        errors.push(coded(&["input", "name"], "Name can't be blank", "BLANK"));
    }
    let options = match input.get("options").and_then(Value::as_array) {
        Some(options) => options.len(),
        None => group
            .and_then(|group| state.get(group))
            .and_then(|record| record.get("options"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
    };
    if creating && options == 0 {
        errors.push(coded(
            &["input", "options"],
            "Options can't be blank",
            "BLANK",
        ));
    }
    if options > 3 {
        errors.push(coded(
            &["input", "options"],
            "Options is too long (maximum is 3)",
            "TOO_LONG",
        ));
    }
    let existing = group
        .and_then(|group| state.get(group))
        .map(|record| gids(record.get("sellingPlans")))
        .unwrap_or_default();
    let to_create = objects(input, "sellingPlansToCreate");
    let to_update = objects(input, "sellingPlansToUpdate");
    let to_delete = gids(input.get("sellingPlansToDelete"));
    for (index, plan) in to_create.iter().enumerate() {
        let path = vec![
            "input".to_string(),
            "sellingPlansToCreate".to_string(),
            index.to_string(),
        ];
        check_plan(plan, options, true, &path, errors);
        check_plan_metafields(state, None, plan, &path, errors);
    }
    for (index, plan) in to_update.iter().enumerate() {
        let path = vec![
            "input".to_string(),
            "sellingPlansToUpdate".to_string(),
            index.to_string(),
        ];
        match text(plan, "id") {
            None => errors.push(coded(
                &["input", "sellingPlansToUpdate", &index.to_string(), "id"],
                "Selling plan ID must be specified to update.",
                "PLAN_ID_MUST_BE_SPECIFIED_TO_UPDATE",
            )),
            Some(id) if !existing.iter().any(|known| known == id) => errors.push(coded(
                &["input", "sellingPlansToUpdate", &index.to_string(), "id"],
                "Selling plan does not exist.",
                "PLAN_DOES_NOT_EXIST",
            )),
            Some(id) => {
                check_plan(plan, options, false, &path, errors);
                check_plan_metafields(state, Some(id), plan, &path, errors);
            }
        }
    }
    for (index, id) in to_delete.iter().enumerate() {
        if !existing.contains(id) {
            errors.push(coded(
                &["input", "sellingPlansToDelete", &index.to_string()],
                "Selling plan does not exist.",
                "PLAN_DOES_NOT_EXIST",
            ));
        }
    }
    let remaining = existing
        .iter()
        .filter(|plan| !to_delete.contains(plan))
        .count()
        + to_create.len();
    if remaining == 0 {
        errors.push(coded(
            &["input", "sellingPlansToCreate"],
            "Must include at least one selling plan.",
            "SELLING_PLAN_COUNT_LOWER_BOUND",
        ));
    } else if remaining > PLAN_LIMIT {
        errors.push(coded(
            &["input", "sellingPlansToCreate"],
            "Exceeded the selling plan limit (31).",
            "SELLING_PLAN_COUNT_UPPER_BOUND",
        ));
    }
    // Plan names are unique in a group.
    let mut names: Vec<String> = existing
        .iter()
        .filter(|plan| !to_delete.contains(plan))
        .filter(|plan| {
            !to_update.iter().any(|update| {
                text(update, "id") == Some(plan.as_str()) && update.contains_key("name")
            })
        })
        .filter_map(|plan| {
            state
                .get(plan)
                .and_then(|record| field(record, "name"))
                .map(str::to_lowercase)
        })
        .collect();
    for plan in to_update.iter().chain(&to_create) {
        if let Some(name) = text(plan, "name").map(str::to_lowercase) {
            if names.contains(&name) {
                errors.push(coded(
                    &["input"],
                    "Cannot have multiple selling plans with the same name.",
                    "SELLING_PLAN_DUPLICATE_NAME",
                ));
            }
            names.push(name);
        }
    }
}

/// `sellingPlanGroupCreate`.
fn group_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let resources = ctx.arg_object_or_default("resources");
    let products = gids(resources.get("productIds"));
    let variants = gids(resources.get("productVariantIds"));
    let state = mutation.state();
    let mut errors = Vec::new();
    check_group(state, &input, None, &mut errors);
    check_resources(state, &products, &variants, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"sellingPlanGroup": null}), &errors));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let id = state.new_gid("SellingPlanGroup");
    let plans = create_plans(state, &objects(&input, "sellingPlansToCreate"), 1);
    let name = text(&input, "name").unwrap_or_default().trim().to_string();
    let app_id = input.get("appId").cloned().unwrap_or(Value::Null);
    state.insert(value::object(json!({
        "__typename": "SellingPlanGroup",
        "id": id,
        "name": name,
        "merchantCode": text(&input, "merchantCode").map_or_else(|| lsf_core::util::handleize(&name), str::to_string),
        "appId": app_id,
        "description": input.get("description"),
        "options": input.get("options").cloned().unwrap_or(json!([])),
        "position": input.get("position"),
        "summary": summary(state, &plans),
        "createdAt": now,
        "products": [],
        "productVariants": [],
        "sellingPlans": plans,
    })));
    link(state, &id, "products", &products, true);
    link(state, &id, "productVariants", &variants, true);
    state.emit("selling_plan_groups/create", &id);
    Ok(payload(json!({"sellingPlanGroup": id}), &[]))
}

/// The group of `id`, or its error.
fn group_arg(state: &ShopState, ctx: &FieldContext<'_>) -> Result<String, UserError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    if state.is_of(id, "SellingPlanGroup") {
        Ok(id.to_string())
    } else {
        Err(coded(
            &["id"],
            "Selling plan group does not exist.",
            "GROUP_DOES_NOT_EXIST",
        ))
    }
}

/// `sellingPlanGroupUpdate`.
fn group_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"sellingPlanGroup": null, "deletedSellingPlanIds": null});
    let state = mutation.state();
    let group = match group_arg(state, ctx) {
        Ok(group) => group,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let input = ctx.arg_object_or_default("input");
    let mut errors = Vec::new();
    check_group(state, &input, Some(&group), &mut errors);
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let to_delete = gids(input.get("sellingPlansToDelete"));
    for plan in &to_delete {
        state.remove(plan);
    }
    for plan in objects(&input, "sellingPlansToUpdate") {
        let id = text(plan, "id").unwrap_or_default().to_string();
        let mut record = state.get(&id).cloned().unwrap_or_default();
        plan_record(state, &mut record, plan, &now);
        state.insert(record);
        let metafields = objects(plan, "metafields");
        records::set_metafields(state, &id, "SELLING_PLAN", &metafields);
    }
    let mut plans = state
        .get(&group)
        .map(|record| gids(record.get("sellingPlans")))
        .unwrap_or_default();
    plans.retain(|plan| !to_delete.contains(plan));
    let created = create_plans(
        state,
        &objects(&input, "sellingPlansToCreate"),
        plans.len() + 1,
    );
    plans.extend(created);
    let summary = summary(state, &plans);
    if let Some(record) = state.get_mut(&group) {
        for name in [
            "name",
            "merchantCode",
            "appId",
            "description",
            "options",
            "position",
        ] {
            if let Some(given) = input.get(name) {
                record.insert(name.into(), given.clone());
            }
        }
        record.insert("sellingPlans".into(), json!(plans));
        record.insert("summary".into(), json!(summary));
    }
    state.emit("selling_plan_groups/update", &group);
    let deleted = (!to_delete.is_empty()).then_some(to_delete);
    Ok(payload(
        json!({"sellingPlanGroup": group, "deletedSellingPlanIds": deleted}),
        &[],
    ))
}

/// `sellingPlanGroupDelete`.
fn group_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let group = match group_arg(state, ctx) {
        Ok(group) => group,
        Err(error) => {
            return Ok(payload(
                json!({"deletedSellingPlanGroupId": null}),
                &[error],
            ));
        }
    };
    let state = mutation.state_mut();
    let record = state.get(&group).cloned().unwrap_or_default();
    link(
        state,
        &group,
        "products",
        &gids(record.get("products")),
        false,
    );
    link(
        state,
        &group,
        "productVariants",
        &gids(record.get("productVariants")),
        false,
    );
    for plan in gids(record.get("sellingPlans")) {
        let metafields = state
            .get(&plan)
            .map(|record| gids(record.get("metafields")))
            .unwrap_or_default();
        for metafield in metafields {
            state.remove(&metafield);
        }
        state.remove(&plan);
    }
    state.delete(&group, "selling_plan_groups/delete");
    Ok(payload(json!({"deletedSellingPlanGroupId": group}), &[]))
}

/// The add and remove mutations: `list` is `products` or `productVariants`, `argument` the
/// argument of their GIDs.
fn change_resources(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    list: &str,
    argument: &str,
    member: bool,
    answer_field: &str,
) -> Result<Value, FieldError> {
    let empty = json!({answer_field: null});
    let state = mutation.state();
    let group = match group_arg(state, ctx) {
        Ok(group) => group,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let ids = gids(ctx.arg(argument));
    let (type_name, message, code) = if list == "products" {
        (
            "Product",
            "Product does not exist.",
            "PRODUCT_DOES_NOT_EXIST",
        )
    } else {
        (
            "ProductVariant",
            "Product variant does not exist.",
            "PRODUCT_VARIANT_DOES_NOT_EXIST",
        )
    };
    let errors: Vec<UserError> = ids
        .iter()
        .enumerate()
        .filter(|(_, id)| !state.is_of(id, type_name))
        .map(|(index, _)| coded(&[argument, &index.to_string()], message, code))
        .collect();
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    if link(state, &group, list, &ids, member) {
        state.emit("selling_plan_groups/update", &group);
    }
    let answer = if member { json!(group) } else { json!(ids) };
    Ok(payload(json!({answer_field: answer}), &[]))
}

fn add_products(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    change_resources(
        mutation,
        ctx,
        "products",
        "productIds",
        true,
        "sellingPlanGroup",
    )
}

fn remove_products(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    change_resources(
        mutation,
        ctx,
        "products",
        "productIds",
        false,
        "removedProductIds",
    )
}

fn add_variants(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    change_resources(
        mutation,
        ctx,
        "productVariants",
        "productVariantIds",
        true,
        "sellingPlanGroup",
    )
}

fn remove_variants(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    change_resources(
        mutation,
        ctx,
        "productVariants",
        "productVariantIds",
        false,
        "removedProductVariantIds",
    )
}

/// `productJoinSellingPlanGroups` and the three others: a product or a variant joins or leaves
/// groups, as `sellingPlanGroupAddProducts` and the others do from the group's side.
fn join_groups(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    type_name: &str,
    member: bool,
) -> Result<Value, FieldError> {
    let (answer, list, message, code) = if type_name == "Product" {
        (
            "product",
            "products",
            "Product does not exist.",
            "PRODUCT_DOES_NOT_EXIST",
        )
    } else {
        (
            "productVariant",
            "productVariants",
            "Product variant does not exist.",
            "PRODUCT_VARIANT_DOES_NOT_EXIST",
        )
    };
    let state = mutation.state();
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if !state.is_of(&id, type_name) {
        return Ok(payload(
            json!({answer: null}),
            &[coded(&["id"], message, code)],
        ));
    }
    let groups = gids(ctx.arg("sellingPlanGroupIds"));
    let errors: Vec<UserError> = groups
        .iter()
        .enumerate()
        .filter(|(_, group)| !state.is_of(group, "SellingPlanGroup"))
        .map(|(index, _)| {
            coded(
                &["sellingPlanGroupIds", &index.to_string()],
                "Selling plan group does not exist.",
                "GROUP_DOES_NOT_EXIST",
            )
        })
        .collect();
    if !errors.is_empty() {
        return Ok(payload(json!({answer: null}), &errors));
    }
    let state = mutation.state_mut();
    for group in &groups {
        if link(state, group, list, std::slice::from_ref(&id), member) {
            state.emit("selling_plan_groups/update", group);
        }
    }
    Ok(payload(json!({answer: id}), &[]))
}

fn product_join(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    join_groups(mutation, ctx, "Product", true)
}

fn product_leave(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    join_groups(mutation, ctx, "Product", false)
}

fn variant_join(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    join_groups(mutation, ctx, "ProductVariant", true)
}

fn variant_leave(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    join_groups(mutation, ctx, "ProductVariant", false)
}
