//! `selling_plan_group`, `selling_plan` and `selling_plan_allocation`: subscriptions and the
//! other purchase options of a product.

use std::any::Any;

use lsf_liquid::{Hash, Object, Value};
use serde_json::{Value as Json, json};

use super::{SiteRef, hash};
use crate::store::{
    Allocation, CheckoutCharge, Product, SellingPlan, SellingPlanGroup, Store, Variant,
};

/// An object with fixed properties, whose JSON is not the list of those properties: the
/// `json` filter prints selling plans as `/products/<handle>.js` does.
struct Fields {
    type_name: &'static str,
    fields: Hash,
    json: Json,
}

impl Object for Fields {
    fn type_name(&self) -> &str {
        self.type_name
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.fields.get(key).cloned()
    }

    fn to_json(&self) -> Json {
        self.json.clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn fields<const N: usize>(type_name: &'static str, pairs: [(&str, Value); N], json: Json) -> Value {
    Value::object(Fields {
        type_name,
        fields: pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
        json,
    })
}

/// The id of the selling plan the URL selects (`?selling_plan=<id>`).
fn selected_id(site: &SiteRef) -> Option<u64> {
    site.request.param("selling_plan")?.parse().ok()
}

/// The selling plan the URL selects, when the product is sold with it.
pub fn selected_plan<'a>(
    site: &'a SiteRef,
    product: &Product,
) -> Option<(&'a SellingPlanGroup, &'a SellingPlan)> {
    site.store.selling_plan_of(product, selected_id(site)?)
}

/// Every selling plan a product is sold with, with its group, in order.
pub fn plans<'a>(
    store: &'a Store,
    product: &'a Product,
) -> impl Iterator<Item = (&'a SellingPlanGroup, &'a SellingPlan)> {
    product.selling_plan_groups.iter().flat_map(move |group| {
        let group = &store.selling_plan_groups[*group];
        group.plans.iter().map(move |plan| (group, plan))
    })
}

fn checkout_charge(plan: &SellingPlan) -> (&'static str, i64) {
    match plan.checkout_charge {
        CheckoutCharge::Percentage(value) => ("percentage", value),
        CheckoutCharge::Price(value) => ("price", value),
    }
}

/// A selling plan as `/products/<handle>.js` lists it in a group.
pub fn plan_json(group: &SellingPlanGroup, plan: &SellingPlan) -> Json {
    let (charge_type, charge) = checkout_charge(plan);
    json!({
        "id": plan.id,
        "name": plan.name,
        "description": plan.description,
        "options": group
            .options
            .iter()
            .zip(&plan.options)
            .enumerate()
            .map(|(index, (name, value))| json!({"name": name, "position": index + 1, "value": value}))
            .collect::<Vec<_>>(),
        "recurring_deliveries": plan.recurring_deliveries,
        "price_adjustments": plan
            .price_adjustments
            .iter()
            .enumerate()
            .map(|(index, adjustment)| {
                let (value_type, value) = adjustment.adjustment.describe();
                json!({
                    "order_count": adjustment.order_count,
                    "position": index + 1,
                    "value_type": value_type,
                    "value": value,
                })
            })
            .collect::<Vec<_>>(),
        "checkout_charge": {"value_type": charge_type, "value": charge},
    })
}

/// The values of one option of a group, in the order the plans give them.
fn option_values(group: &SellingPlanGroup, index: usize) -> Vec<&str> {
    let mut values: Vec<&str> = Vec::new();
    for plan in &group.plans {
        if let Some(value) = plan.options.get(index)
            && !values.contains(&value.as_str())
        {
            values.push(value);
        }
    }
    values
}

/// A selling plan group as `/products/<handle>.js` lists it.
pub fn group_json(group: &SellingPlanGroup) -> Json {
    json!({
        "id": group.id,
        "name": group.name,
        "options": group
            .options
            .iter()
            .enumerate()
            .map(|(index, name)| json!({
                "name": name,
                "position": index + 1,
                "values": option_values(group, index),
            }))
            .collect::<Vec<_>>(),
        "selling_plans": group
            .plans
            .iter()
            .map(|plan| plan_json(group, plan))
            .collect::<Vec<_>>(),
        "app_id": group.app_id,
    })
}

fn adjusted_prices_json(allocation: &Allocation) -> Vec<Json> {
    allocation
        .adjusted_prices
        .iter()
        .enumerate()
        .map(|(index, price)| json!({"position": index + 1, "price": price}))
        .collect()
}

/// An allocation as `/products/<handle>.js` lists it on a variant.
pub fn allocation_json(group: &SellingPlanGroup, plan: &SellingPlan, variant: &Variant) -> Json {
    let allocation = plan.allocation(variant);
    json!({
        "price_adjustments": adjusted_prices_json(&allocation),
        "price": allocation.price,
        "compare_at_price": allocation.compare_at_price,
        "per_delivery_price": allocation.per_delivery_price,
        "selling_plan_id": plan.id,
        "selling_plan_group_id": group.id,
    })
}

/// An allocation as `/cart.js` describes it on a line: with the plan itself.
pub fn line_allocation_json(
    group: &SellingPlanGroup,
    plan: &SellingPlan,
    variant: &Variant,
) -> Json {
    let allocation = plan.allocation(variant);
    let mut selling_plan = plan_json(group, plan);
    if let Some(object) = selling_plan.as_object_mut() {
        object.remove("checkout_charge");
        object.insert("fixed_selling_plan".to_string(), json!(false));
    }
    json!({
        "price_adjustments": adjusted_prices_json(&allocation),
        "price": allocation.price,
        "compare_at_price": allocation.compare_at_price,
        "per_delivery_price": allocation.per_delivery_price,
        "selling_plan": selling_plan,
    })
}

/// The `selling_plan` object.
pub fn plan_value(site: &SiteRef, group: &SellingPlanGroup, plan: &SellingPlan) -> Value {
    let (charge_type, charge) = checkout_charge(plan);
    fields(
        "selling_plan",
        [
            ("id", Value::Int(plan.id as i64)),
            ("name", Value::from(&plan.name)),
            (
                "description",
                plan.description.as_ref().map_or(Value::Nil, Value::from),
            ),
            ("group_id", Value::from(&group.id)),
            (
                "options",
                Value::array(
                    group
                        .options
                        .iter()
                        .zip(&plan.options)
                        .enumerate()
                        .map(|(index, (name, value))| {
                            hash([
                                ("name", Value::from(name)),
                                ("position", Value::from(index + 1)),
                                ("value", Value::from(value)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "price_adjustments",
                Value::array(
                    plan.price_adjustments
                        .iter()
                        .enumerate()
                        .map(|(index, adjustment)| {
                            let (value_type, value) = adjustment.adjustment.describe();
                            hash([
                                (
                                    "order_count",
                                    adjustment.order_count.map_or(Value::Nil, Value::from),
                                ),
                                ("position", Value::from(index + 1)),
                                ("value", Value::Int(value)),
                                ("value_type", Value::str(value_type)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "recurring_deliveries",
                Value::Bool(plan.recurring_deliveries),
            ),
            (
                "checkout_charge",
                hash([
                    ("value", Value::Int(charge)),
                    ("value_type", Value::str(charge_type)),
                ]),
            ),
            ("selected", Value::Bool(selected_id(site) == Some(plan.id))),
        ],
        plan_json(group, plan),
    )
}

/// The `selling_plan_group` object.
pub fn group_value(site: &SiteRef, group: &SellingPlanGroup) -> Value {
    let selected = selected_id(site).and_then(|id| group.plans.iter().find(|plan| plan.id == id));
    fields(
        "selling_plan_group",
        [
            ("id", Value::from(&group.id)),
            ("name", Value::from(&group.name)),
            (
                "app_id",
                group.app_id.as_ref().map_or(Value::Nil, Value::from),
            ),
            (
                "options",
                Value::array(
                    group
                        .options
                        .iter()
                        .enumerate()
                        .map(|(index, name)| {
                            hash([
                                ("name", Value::from(name)),
                                ("position", Value::from(index + 1)),
                                (
                                    "selected_value",
                                    selected
                                        .and_then(|plan| plan.options.get(index))
                                        .map_or(Value::Nil, Value::from),
                                ),
                                (
                                    "values",
                                    Value::array(
                                        option_values(group, index)
                                            .into_iter()
                                            .map(Value::from)
                                            .collect(),
                                    ),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("selling_plan_selected", Value::Bool(selected.is_some())),
            (
                "selling_plans",
                Value::array(
                    group
                        .plans
                        .iter()
                        .map(|plan| plan_value(site, group, plan))
                        .collect(),
                ),
            ),
        ],
        group_json(group),
    )
}

/// The `selling_plan_allocation` object: what a variant costs with a selling plan.
pub fn allocation_value(
    site: &SiteRef,
    group: &SellingPlanGroup,
    plan: &SellingPlan,
    variant: &Variant,
) -> Value {
    let allocation = plan.allocation(variant);
    fields(
        "selling_plan_allocation",
        [
            ("price", Value::Int(allocation.price)),
            (
                "compare_at_price",
                allocation.compare_at_price.map_or(Value::Nil, Value::Int),
            ),
            (
                "per_delivery_price",
                Value::Int(allocation.per_delivery_price),
            ),
            (
                "checkout_charge_amount",
                Value::Int(allocation.checkout_charge_amount),
            ),
            (
                "remaining_balance_charge_amount",
                Value::Int(allocation.remaining_balance_charge_amount),
            ),
            (
                "unit_price",
                allocation.unit_price.map_or(Value::Nil, Value::Int),
            ),
            (
                "price_adjustments",
                Value::array(
                    allocation
                        .adjusted_prices
                        .iter()
                        .enumerate()
                        .map(|(index, price)| {
                            hash([
                                ("position", Value::from(index + 1)),
                                ("price", Value::Int(*price)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("selling_plan", plan_value(site, group, plan)),
            ("selling_plan_group_id", Value::from(&group.id)),
        ],
        allocation_json(group, plan, variant),
    )
}

/// The allocations of a variant: one per selling plan its product is sold with.
pub fn allocations_value(site: &SiteRef, product: &Product, variant: &Variant) -> Value {
    Value::array(
        plans(&site.store, product)
            .map(|(group, plan)| allocation_value(site, group, plan, variant))
            .collect(),
    )
}
