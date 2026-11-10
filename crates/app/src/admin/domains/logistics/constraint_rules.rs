//! Fulfillment constraint rules: `fulfillmentConstraintRuleCreate`, `…Update`, `…Delete`, and
//! `fulfillmentConstraintRules` (every rule, a list the generic rules answer).
//!
//! A rule runs a Function of the Fulfillment Constraints API (`apiType`
//! `fulfillment_constraints`) of the app, named by `functionHandle` (or the deprecated
//! `functionId`: one of them, `MISSING_FUNCTION_IDENTIFIER`, `MULTIPLE_FUNCTION_IDENTIFIERS`),
//! for the delivery method types given, with its metafields (the Function's configuration). One
//! rule per Function (`FUNCTION_ALREADY_REGISTERED`), ten at most
//! (`MAXIMUM_FULFILLMENT_CONSTRAINT_RULES_REACHED`: "Limit is 10."). The messages are the
//! descriptions of `FulfillmentConstraintRule…UserErrorCode`. Deleting a rule deletes its
//! metafields ("Deletes a fulfillment constraint rule and its metafields").
//!
//! Shopify has no webhook topic for rules: their changes are `domains::app::CHANGED`. The local
//! checkout does not run fulfillment constraint Functions, so a rule changes nothing there.

use serde_json::{Value, json};

use super::coded;
use crate::admin::domains::app::functions::{FunctionProblem, function_of};
use crate::admin::domains::app::{self, metafields as app_metafields};
use crate::admin::graphql::{FieldContext, FieldError};
use crate::admin::resolve::{Mutation, Registry, payload};
use crate::admin::state::value;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("fulfillmentConstraintRuleCreate", create)
        .mutation("fulfillmentConstraintRuleUpdate", update)
        .mutation("fulfillmentConstraintRuleDelete", delete);
}

/// The most rules a shop has.
const LIMIT: usize = 10;

/// The `MetafieldOwnerType` of a rule.
const OWNER_TYPE: &str = "FULFILLMENT_CONSTRAINT_RULE";

fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"fulfillmentConstraintRule": null});
    let arguments = ctx.arguments().clone();
    let state = mutation.state();
    let function = match function_of(state, &arguments, &["fulfillment_constraints"]) {
        Ok(function) => function,
        Err(problem) => {
            let (field, code) = match problem {
                FunctionProblem::Missing => ("functionHandle", "MISSING_FUNCTION_IDENTIFIER"),
                FunctionProblem::Multiple => ("functionHandle", "MULTIPLE_FUNCTION_IDENTIFIERS"),
                FunctionProblem::NotFound { field, .. } => (field, "FUNCTION_NOT_FOUND"),
                FunctionProblem::DoesNotImplement { field, .. } => {
                    (field, "FUNCTION_DOES_NOT_IMPLEMENT")
                }
            };
            return Ok(payload(empty, &[coded(ctx, &[field], code)]));
        }
    };
    if state
        .all("FulfillmentConstraintRule")
        .any(|rule| rule.get("function").and_then(Value::as_str) == Some(function.id.as_str()))
    {
        return Ok(payload(
            empty,
            &[coded(
                ctx,
                &["functionHandle"],
                "FUNCTION_ALREADY_REGISTERED",
            )],
        ));
    }
    if state.count("FulfillmentConstraintRule") >= LIMIT {
        return Ok(payload(
            empty,
            &[coded(
                ctx,
                &[],
                "MAXIMUM_FULFILLMENT_CONSTRAINT_RULES_REACHED",
            )],
        ));
    }
    let types = ctx.arg("deliveryMethodTypes").cloned().unwrap_or(json!([]));
    if types.as_array().is_none_or(Vec::is_empty) {
        return Ok(payload(
            empty,
            &[coded(ctx, &["deliveryMethodTypes"], "INPUT_INVALID")],
        ));
    }
    let metafields = app_metafields::inputs_of(&arguments);
    let mut errors = Vec::new();
    app_metafields::check(
        state,
        None,
        OWNER_TYPE,
        &metafields,
        &[],
        Some("INPUT_INVALID"),
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let id = state.new_gid("FulfillmentConstraintRule");
    state.insert(value::object(json!({
        "__typename": "FulfillmentConstraintRule",
        "id": id,
        "deliveryMethodTypes": types,
        "function": function.id,
        "metafields": [],
    })));
    app_metafields::apply(state, &id, OWNER_TYPE, &metafields);
    state.emit(app::CHANGED, &id);
    Ok(payload(json!({"fulfillmentConstraintRule": id}), &[]))
}

/// The rule of the `id` argument.
fn rule_of(mutation: &Mutation<'_>, ctx: &FieldContext<'_>) -> Option<String> {
    let id = ctx.arg_str("id")?;
    (mutation.state().type_of(id) == Some("FulfillmentConstraintRule")).then(|| id.to_string())
}

fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let Some(id) = rule_of(mutation, ctx) else {
        return Ok(payload(
            json!({"fulfillmentConstraintRule": null}),
            &[coded(ctx, &["id"], "NOT_FOUND")],
        ));
    };
    let types = ctx.arg("deliveryMethodTypes").cloned().unwrap_or(json!([]));
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("deliveryMethodTypes".into(), types);
    }
    state.emit(app::CHANGED, &id);
    Ok(payload(json!({"fulfillmentConstraintRule": id}), &[]))
}

fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let Some(id) = rule_of(mutation, ctx) else {
        return Ok(payload(
            json!({"success": null}),
            &[coded(ctx, &["id"], "NOT_FOUND")],
        ));
    };
    let state = mutation.state_mut();
    app_metafields::remove_all(state, &id);
    state.emit(app::CHANGED, &id);
    state.remove(&id);
    Ok(payload(json!({"success": true}), &[]))
}
