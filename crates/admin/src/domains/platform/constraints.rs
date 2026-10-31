//! Fulfillment constraint rules: `fulfillmentConstraintRuleCreate`, `Update`, `Delete`; the
//! `fulfillmentConstraintRules` query.
//!
//! A rule runs a Function of the app that implements the Fulfillment Constraints API (`apiType`
//! `fulfillment_constraints`), named by `functionHandle` (or the deprecated `functionId`), as
//! the app family's customizations name theirs (`app::functions::function_of`: the same checks
//! and codes, `MISSING_FUNCTION_IDENTIFIER`, `MULTIPLE_FUNCTION_IDENTIFIERS`,
//! `FUNCTION_NOT_FOUND`, `FUNCTION_DOES_NOT_IMPLEMENT`), for `deliveryMethodTypes`, with its
//! configuration in its metafields (checked as `metafieldsSet` checks them, owner type
//! `FULFILLMENT_CONSTRAINT_RULE`). One rule per Function (`FUNCTION_ALREADY_REGISTERED`). The
//! local checkout does not run fulfillment constraint Functions: the rule is kept and read
//! back. Shopify has no webhook topic for it.

use serde_json::{Value, json};

use crate::domains::app::functions::{FunctionProblem, function_of};
use crate::domains::app::metafields;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::value;

const OWNER_TYPE: &str = "FULFILLMENT_CONSTRAINT_RULE";

pub fn register(registry: &mut Registry) {
    registry
        .mutation("fulfillmentConstraintRuleCreate", create)
        .mutation("fulfillmentConstraintRuleUpdate", update)
        .mutation("fulfillmentConstraintRuleDelete", delete)
        .query("fulfillmentConstraintRules", rules);
}

fn methods(ctx: &FieldContext<'_>) -> Vec<Value> {
    ctx.arg("deliveryMethodTypes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let refuse = |field: &str, message: &str, code: &str| {
        Ok(payload(
            json!({"fulfillmentConstraintRule": null}),
            &[UserError::new(&[field], message).code(code)],
        ))
    };
    let function = match function_of(m.state(), ctx.arguments(), &["fulfillment_constraints"]) {
        Ok(function) => function,
        Err(FunctionProblem::Missing) => {
            return refuse(
                "functionHandle",
                "Either function_id or function_handle must be provided.",
                "MISSING_FUNCTION_IDENTIFIER",
            );
        }
        Err(FunctionProblem::Multiple) => {
            return refuse(
                "functionHandle",
                "Only one of function_id or function_handle can be provided, not both.",
                "MULTIPLE_FUNCTION_IDENTIFIERS",
            );
        }
        Err(FunctionProblem::NotFound { field, .. }) => {
            return refuse(field, "Function not found.", "FUNCTION_NOT_FOUND");
        }
        Err(FunctionProblem::DoesNotImplement { field, .. }) => {
            return refuse(
                field,
                "Function does not implement the required interface for this fulfillment constraint rule.",
                "FUNCTION_DOES_NOT_IMPLEMENT",
            );
        }
    };
    if m.state()
        .all("FulfillmentConstraintRule")
        .any(|rule| rule.get("function").and_then(Value::as_str) == Some(function.id.as_str()))
    {
        return refuse(
            "functionHandle",
            "A fulfillment constraint rule already runs this Function.",
            "FUNCTION_ALREADY_REGISTERED",
        );
    }
    let methods = methods(ctx);
    if methods.is_empty() {
        return refuse(
            "deliveryMethodTypes",
            "Delivery method types can't be blank",
            "INPUT_INVALID",
        );
    }
    let inputs: Vec<Value> = ctx
        .arg("metafields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut errors = Vec::new();
    metafields::check(
        m.state(),
        None,
        OWNER_TYPE,
        &inputs,
        &[],
        Some("INPUT_INVALID"),
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"fulfillmentConstraintRule": null}), &errors));
    }
    let state = m.state_mut();
    let id = state.insert(value::object(json!({
        "__typename": "FulfillmentConstraintRule",
        "deliveryMethodTypes": methods,
        "function": function.id,
        "metafields": [],
    })));
    metafields::apply(state, &id, OWNER_TYPE, &inputs);
    state.emit(crate::domains::app::CHANGED, &id);
    Ok(payload(json!({"fulfillmentConstraintRule": id}), &[]))
}

fn rule_of(m: &Mutation<'_>, id: &str) -> bool {
    m.state().type_of(id) == Some("FulfillmentConstraintRule")
}

fn update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if !rule_of(m, &id) {
        let error = UserError::new(&["id"], "Could not find fulfillment constraint rule.")
            .code("NOT_FOUND");
        return Ok(payload(
            json!({"fulfillmentConstraintRule": null}),
            &[error],
        ));
    }
    let methods = methods(ctx);
    let state = m.state_mut();
    if let Some(rule) = state.get_mut(&id) {
        rule.insert("deliveryMethodTypes".into(), Value::Array(methods));
    }
    state.emit(crate::domains::app::CHANGED, &id);
    Ok(payload(json!({"fulfillmentConstraintRule": id}), &[]))
}

fn delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if !rule_of(m, &id) {
        let error = UserError::new(&["id"], "Could not find fulfillment constraint rule.")
            .code("NOT_FOUND");
        return Ok(payload(json!({"success": null}), &[error]));
    }
    let state = m.state_mut();
    metafields::remove_all(state, &id);
    state.delete(&id, crate::domains::app::CHANGED);
    Ok(payload(json!({"success": true}), &[]))
}

fn rules(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = Value::Array(r.records_of("FulfillmentConstraintRule"));
    r.convert(ctx, &items, true)
}
