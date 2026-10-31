//! The app's Shopify Functions and the objects that run them at the checkout: payment and
//! delivery customizations, cart and checkout validations, cart transforms.
//!
//! - **Queries**: `shopifyFunctions(apiType:, useCreationUi:)` (the app's, as Shopify lists only
//!   those of the calling app), `shopifyFunction(id:)`, and the root connections of the owners,
//!   which the generic rules answer from the records.
//! - **Mutations**: `paymentCustomizationCreate`/`Update`/`Delete`/`Activation`, the same four of
//!   delivery customizations, `validationCreate`/`Update`/`Delete`, `cartTransformCreate`/
//!   `Delete`. A Function is named by `functionHandle` (2026-10's) or the deprecated
//!   `functionId`, never both; it must be one of the app's, and implement the API of the owner
//!   (its `apiType`). The owner's `metafields` are the configuration the Function reads: the
//!   store sync writes the metafields of the enabled owners of a Function back as its
//!   `checkout.functions` settings.
//!
//! The error codes are those of 2026-10 (`PaymentCustomizationErrorCode`, …); their messages are
//! the descriptions of the codes ("Function not found.", which forum answers quote word for word
//! for these mutations), Shopify's wording for the others not being published. The limits are
//! those the codes describe: 25 active validations ("Cannot have more than 25 active validation
//! functions."), one cart transform per Function ("A cart transform function already exists for
//! the provided function_id."); lsf allows any number of payment and delivery customizations.
//!
//! None of these has a webhook topic: a change emits [`super::CHANGED`], which no subscription
//! receives, so that the server knows the shop changed.

use serde_json::{Map, Value, json};

use super::{CHANGED, metafields};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .query("shopifyFunctions", shopify_functions)
        .mutation("paymentCustomizationCreate", |m, ctx| {
            create(m, ctx, &PAYMENT)
        })
        .mutation("paymentCustomizationUpdate", |m, ctx| {
            update(m, ctx, &PAYMENT)
        })
        .mutation("paymentCustomizationDelete", |m, ctx| {
            delete(m, ctx, &PAYMENT)
        })
        .mutation("paymentCustomizationActivation", |m, ctx| {
            activation(m, ctx, &PAYMENT)
        })
        .mutation("deliveryCustomizationCreate", |m, ctx| {
            create(m, ctx, &DELIVERY)
        })
        .mutation("deliveryCustomizationUpdate", |m, ctx| {
            update(m, ctx, &DELIVERY)
        })
        .mutation("deliveryCustomizationDelete", |m, ctx| {
            delete(m, ctx, &DELIVERY)
        })
        .mutation("deliveryCustomizationActivation", |m, ctx| {
            activation(m, ctx, &DELIVERY)
        })
        .mutation("validationCreate", |m, ctx| create(m, ctx, &VALIDATION))
        .mutation("validationUpdate", |m, ctx| update(m, ctx, &VALIDATION))
        .mutation("validationDelete", |m, ctx| delete(m, ctx, &VALIDATION))
        .mutation("cartTransformCreate", |m, ctx| {
            create(m, ctx, &CART_TRANSFORM)
        })
        .mutation("cartTransformDelete", |m, ctx| {
            delete(m, ctx, &CART_TRANSFORM)
        });
}

/// A Function of the app, as an input names it.
#[derive(Clone, Debug)]
pub struct FunctionRef {
    /// Its id, the key of its `ShopifyFunction` record.
    pub id: String,
    pub handle: String,
    pub api_type: String,
    pub title: String,
}

/// Why an input names no Function the owner can run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FunctionProblem {
    /// Neither `functionHandle` nor `functionId`.
    Missing,
    /// Both.
    Multiple,
    /// No Function of the app has that handle (`functionHandle`) or id (`functionId`).
    NotFound { field: &'static str, value: String },
    /// The Function implements another API.
    DoesNotImplement {
        field: &'static str,
        api_type: String,
    },
}

/// The Function an input names (`functionHandle` or `functionId`), checked against the app's
/// Functions and the API types the owner runs.
pub fn function_of(
    state: &ShopState,
    input: &Map<String, Value>,
    api_types: &[&str],
) -> Result<FunctionRef, FunctionProblem> {
    let given = |name: &str| {
        input
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
    };
    let (field, value) = match (given("functionHandle"), given("functionId")) {
        (Some(_), Some(_)) => return Err(FunctionProblem::Multiple),
        (None, None) => return Err(FunctionProblem::Missing),
        (Some(handle), None) => ("functionHandle", handle),
        (None, Some(id)) => ("functionId", id),
    };
    let found = state.all("ShopifyFunction").find(|function| {
        let text = |name: &str| function.get(name).and_then(Value::as_str);
        match field {
            "functionHandle" => text("handle") == Some(value),
            _ => text("id") == Some(value),
        }
    });
    let Some(function) = found else {
        return Err(FunctionProblem::NotFound {
            field,
            value: value.to_string(),
        });
    };
    let text = |name: &str| {
        function
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let api_type = text("apiType");
    if !api_types.contains(&api_type.as_str()) {
        return Err(FunctionProblem::DoesNotImplement { field, api_type });
    }
    Ok(FunctionRef {
        id: text("id"),
        handle: text("handle"),
        api_type,
        title: text("title"),
    })
}

/// What a kind of Function owner is: its type, the argument of its input, the API its Function
/// implements, and the codes and messages of its errors.
struct Owner {
    type_name: &'static str,
    /// The input argument of create and update (`paymentCustomization`); none for a cart
    /// transform, whose arguments are at the root.
    input: Option<&'static str>,
    /// The payload field of the owner (`paymentCustomization`).
    field: &'static str,
    api_type: &'static str,
    owner_type: &'static str,
    /// `enabled` (customizations) or `enable` (validations); none for a cart transform.
    enabled_field: Option<&'static str>,
    not_found: (&'static str, &'static str),
    max_active: Option<(usize, &'static str, &'static str)>,
}

const PAYMENT: Owner = Owner {
    type_name: "PaymentCustomization",
    input: Some("paymentCustomization"),
    field: "paymentCustomization",
    api_type: "payment_customization",
    owner_type: "PAYMENT_CUSTOMIZATION",
    enabled_field: Some("enabled"),
    not_found: (
        "PAYMENT_CUSTOMIZATION_NOT_FOUND",
        "Payment customization not found.",
    ),
    max_active: None,
};

const DELIVERY: Owner = Owner {
    type_name: "DeliveryCustomization",
    input: Some("deliveryCustomization"),
    field: "deliveryCustomization",
    api_type: "delivery_customization",
    owner_type: "DELIVERY_CUSTOMIZATION",
    enabled_field: Some("enabled"),
    not_found: (
        "DELIVERY_CUSTOMIZATION_NOT_FOUND",
        "Delivery customization not found.",
    ),
    max_active: None,
};

const VALIDATION: Owner = Owner {
    type_name: "Validation",
    input: Some("validation"),
    field: "validation",
    api_type: "cart_checkout_validation",
    owner_type: "VALIDATION",
    enabled_field: Some("enable"),
    not_found: ("NOT_FOUND", "Validation not found."),
    max_active: Some((
        25,
        "MAX_VALIDATIONS_ACTIVATED",
        "Cannot have more than 25 active validation functions.",
    )),
};

const CART_TRANSFORM: Owner = Owner {
    type_name: "CartTransform",
    input: None,
    field: "cartTransform",
    api_type: "cart_transform",
    owner_type: "CARTTRANSFORM",
    enabled_field: None,
    not_found: (
        "NOT_FOUND",
        "Could not find cart transform for provided id.",
    ),
    max_active: None,
};

/// The input of a create or an update: the input object, or the root arguments of a cart
/// transform.
fn input_of(ctx: &FieldContext<'_>, owner: &Owner) -> Map<String, Value> {
    match owner.input {
        Some(name) => ctx
            .arg(name)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        None => ctx.arguments().clone(),
    }
}

/// The path of an input field in the errors: `["paymentCustomization", "title"]`, `["title"]`
/// for a cart transform.
fn path<'a>(owner: &'a Owner, field: &'a str) -> Vec<&'a str> {
    owner.input.into_iter().chain([field]).collect()
}

/// The error of a Function an input does not name rightly, in the owner's codes.
fn function_error(owner: &Owner, problem: &FunctionProblem) -> UserError {
    match problem {
        FunctionProblem::Missing => UserError::new(
            &path(owner, "functionHandle"),
            "Either function_id or function_handle must be provided.",
        )
        .code("MISSING_FUNCTION_IDENTIFIER"),
        FunctionProblem::Multiple => UserError::new(
            &path(owner, "functionHandle"),
            "Only one of function_id or function_handle can be provided, not both.",
        )
        .code("MULTIPLE_FUNCTION_IDENTIFIERS"),
        FunctionProblem::NotFound { field, .. } => {
            let message = if owner.type_name == "CartTransform" {
                "No Shopify Function found for provided function_id."
            } else {
                "Function not found."
            };
            UserError::new(&path(owner, field), message).code("FUNCTION_NOT_FOUND")
        }
        FunctionProblem::DoesNotImplement { field, .. } => {
            let message = match owner.type_name {
                "PaymentCustomization" => "Function does not implement the required interface.",
                "DeliveryCustomization" => {
                    "Function does not implement the required interface for this delivery customization."
                }
                "Validation" => {
                    "Function does not implement the required interface for this cart & checkout validation."
                }
                _ => {
                    "Function does not implement the required interface for this cart_transform function."
                }
            };
            UserError::new(&path(owner, field), message).code("FUNCTION_DOES_NOT_IMPLEMENT")
        }
    }
}

/// The owners of a type that are enabled.
fn active_count(state: &ShopState, owner: &Owner, except: Option<&str>) -> usize {
    state
        .keys(owner.type_name)
        .filter(|key| Some(*key) != except)
        .filter(|key| {
            state
                .get(key)
                .and_then(|record| record.get("enabled"))
                .and_then(Value::as_bool)
                == Some(true)
        })
        .count()
}

fn create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    owner: &Owner,
) -> Result<Value, FieldError> {
    let input = input_of(ctx, owner);
    let failed = || json!({ owner.field: null });
    let mut errors = Vec::new();
    let state = mutation.state();
    let function = match function_of(state, &input, &[owner.api_type]) {
        Ok(function) => Some(function),
        Err(problem) => {
            errors.push(function_error(owner, &problem));
            None
        }
    };
    let title = input.get("title").and_then(Value::as_str).map(str::trim);
    let enabled = owner.enabled_field.map(|field| {
        input
            .get(field)
            .and_then(Value::as_bool)
            .unwrap_or_default()
    });
    if owner.type_name != "CartTransform" {
        // Customizations require their title and whether they are enabled
        // (`REQUIRED_INPUT_FIELD`: "Required input field must be present."); a validation's
        // title is optional, `enable` has a default.
        let customization = owner.type_name.ends_with("Customization");
        if customization && title.is_none_or(str::is_empty) {
            errors.push(
                UserError::new(
                    &path(owner, "title"),
                    "Required input field must be present.",
                )
                .code("REQUIRED_INPUT_FIELD"),
            );
        }
        if customization && input.get("enabled").is_none_or(Value::is_null) {
            errors.push(
                UserError::new(
                    &path(owner, "enabled"),
                    "Required input field must be present.",
                )
                .code("REQUIRED_INPUT_FIELD"),
            );
        }
    }
    if let Some(function) = &function
        && owner.type_name == "CartTransform"
        && state.all("CartTransform").any(|record| {
            record.get("functionId").and_then(Value::as_str) == Some(function.id.as_str())
        })
    {
        errors.push(
            UserError::new(
                &["functionHandle"],
                "A cart transform function already exists for the provided function_id.",
            )
            .code("FUNCTION_ALREADY_REGISTERED"),
        );
    }
    if let (Some((limit, code, message)), Some(true)) = (owner.max_active, enabled)
        && active_count(state, owner, None) >= limit
    {
        errors.push(UserError::new(&path(owner, "enable"), message).code(code));
    }
    let inputs = metafields::inputs_of(&input);
    let metafield_code = match owner.type_name {
        "Validation" => "INVALID",
        _ => "INVALID_METAFIELDS",
    };
    metafields::check(
        state,
        None,
        &inputs,
        &owner.input.into_iter().collect::<Vec<_>>(),
        Some(metafield_code),
        &mut errors,
    );
    let Some(function) = function.filter(|_| errors.is_empty()) else {
        return Ok(payload(failed(), &errors));
    };

    let state = mutation.state_mut();
    let id = state.new_gid(owner.type_name);
    let mut record = value::object(json!({
        "__typename": owner.type_name,
        "id": id,
        "functionId": function.id,
        "shopifyFunction": function.id,
        "metafields": [],
        "errorHistory": null,
    }));
    match owner.type_name {
        "CartTransform" => {
            record.remove("shopifyFunction");
            let block = input
                .get("blockOnFailure")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            record.insert("blockOnFailure".into(), json!(block));
        }
        "Validation" => {
            // A validation has no `functionId` field: its Function is `shopifyFunction`.
            record.remove("functionId");
            record.insert(
                "title".into(),
                json!(
                    title
                        .filter(|title| !title.is_empty())
                        .unwrap_or(&function.title)
                ),
            );
            record.insert("enabled".into(), json!(enabled.unwrap_or(false)));
            let block = input
                .get("blockOnFailure")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            record.insert("blockOnFailure".into(), json!(block));
        }
        _ => {
            record.insert("title".into(), json!(title.unwrap_or_default()));
            record.insert("enabled".into(), json!(enabled.unwrap_or(false)));
        }
    }
    state.insert(record);
    metafields::apply(state, &id, owner.owner_type, &inputs);
    state.emit(CHANGED, &id);
    Ok(payload(json!({ owner.field: id }), &[]))
}

/// The owner of an `id` argument, when it is one of the type.
fn owner_record(state: &ShopState, owner: &Owner, id: &str) -> Option<String> {
    (state.type_of(id) == Some(owner.type_name)).then(|| id.to_string())
}

fn update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    owner: &Owner,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = input_of(ctx, owner);
    let failed = || json!({ owner.field: null });
    let state = mutation.state();
    let Some(id) = owner_record(state, owner, &id) else {
        let (code, message) = owner.not_found;
        let error = UserError::new(&["id"], message).code(code);
        return Ok(payload(failed(), &[error]));
    };
    let mut errors = Vec::new();
    let current = state
        .get(&id)
        .and_then(|record| {
            record
                .get("functionId")
                .or_else(|| record.get("shopifyFunction"))
        })
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let names_function = ["functionHandle", "functionId"]
        .iter()
        .any(|name| input.get(*name).is_some_and(|value| !value.is_null()));
    if names_function {
        match function_of(state, &input, &[owner.api_type]) {
            Ok(function) if function.id == current => {}
            Ok(_) => errors.push(
                UserError::new(
                    &path(owner, "functionHandle"),
                    "Function ID cannot be changed.",
                )
                .code("FUNCTION_ID_CANNOT_BE_CHANGED"),
            ),
            Err(problem) => errors.push(function_error(owner, &problem)),
        }
    }
    let enabled = owner
        .enabled_field
        .and_then(|field| input.get(field))
        .and_then(Value::as_bool);
    if let (Some((limit, code, message)), Some(true)) = (owner.max_active, enabled)
        && active_count(state, owner, Some(&id)) >= limit
    {
        errors.push(UserError::new(&path(owner, "enable"), message).code(code));
    }
    let inputs = metafields::inputs_of(&input);
    let metafield_code = match owner.type_name {
        "Validation" => "INVALID",
        _ => "INVALID_METAFIELDS",
    };
    metafields::check(
        state,
        Some(&id),
        &inputs,
        &owner.input.into_iter().collect::<Vec<_>>(),
        Some(metafield_code),
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(failed(), &errors));
    }

    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        if let Some(title) = input.get("title").and_then(Value::as_str) {
            record.insert("title".into(), json!(title.trim()));
        }
        if let Some(enabled) = enabled {
            record.insert("enabled".into(), json!(enabled));
        }
        if let Some(block) = input.get("blockOnFailure").and_then(Value::as_bool) {
            record.insert("blockOnFailure".into(), json!(block));
        }
    }
    metafields::apply(state, &id, owner.owner_type, &inputs);
    state.emit(CHANGED, &id);
    Ok(payload(json!({ owner.field: id }), &[]))
}

fn delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    owner: &Owner,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let Some(id) = owner_record(mutation.state(), owner, &id) else {
        let (code, message) = owner.not_found;
        let error = UserError::new(&["id"], message).code(code);
        return Ok(payload(json!({"deletedId": null}), &[error]));
    };
    let state = mutation.state_mut();
    metafields::remove_all(state, &id);
    state.emit(CHANGED, &id);
    state.remove(&id);
    Ok(payload(json!({ "deletedId": id }), &[]))
}

/// `paymentCustomizationActivation`, `deliveryCustomizationActivation`: enables or disables
/// several, all or none. Answers the ids it changed.
fn activation(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    owner: &Owner,
) -> Result<Value, FieldError> {
    let enabled = ctx.arg_bool("enabled").unwrap_or(false);
    let ids: Vec<String> = ctx
        .arg("ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let state = mutation.state();
    let (code, message) = owner.not_found;
    let errors: Vec<UserError> = ids
        .iter()
        .enumerate()
        .filter(|(_, id)| owner_record(state, owner, id).is_none())
        .map(|(index, _)| UserError::new(&["ids", &index.to_string()], message).code(code))
        .collect();
    if !errors.is_empty() {
        return Ok(payload(json!({"ids": null}), &errors));
    }
    let state = mutation.state_mut();
    for id in &ids {
        if let Some(record) = state.get_mut(id) {
            record.insert("enabled".into(), json!(enabled));
        }
        state.emit(CHANGED, id);
    }
    Ok(payload(json!({ "ids": ids }), &[]))
}

/// `shopifyFunctions(apiType:, useCreationUi:)`: the Functions of the app, filtered.
fn shopify_functions(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let api_type = ctx.arg_str("apiType");
    let creation_ui = ctx.arg_bool("useCreationUi");
    let items: Vec<Value> = r
        .records_of("ShopifyFunction")
        .into_iter()
        .filter(|item| {
            let Some(function) = r.item_map(item) else {
                return false;
            };
            api_type.is_none_or(|wanted| {
                function.get("apiType").and_then(Value::as_str) == Some(wanted)
            }) && creation_ui.is_none_or(|wanted| {
                function.get("useCreationUi").and_then(Value::as_bool) == Some(wanted)
            })
        })
        .collect();
    r.connection(ctx, items)
}
