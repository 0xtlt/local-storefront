//! The deprecated staged upload targets: `stagedUploadTargetGenerate` (one target) and
//! `stagedUploadTargetsGenerate` (several), "Use `stagedUploadsCreate` instead".
//!
//! Each runs the app family's `stagedUploadsCreate` with the same input (its checks, its
//! targets on lsf's upload endpoint, the bytes kept for `fileCreate` and the bulk operations)
//! and answers in the deprecated shapes: `url` and `parameters`, or `urls` of
//! `StagedUploadTarget`s.

use serde_json::{Map, Value, json};

use crate::graphql::{self, FieldContext, FieldError, Request};
use crate::resolve::{Mutation, Registry};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("stagedUploadTargetGenerate", target_generate)
        .mutation("stagedUploadTargetsGenerate", targets_generate);
}

/// The targets and the user errors of `stagedUploadsCreate(input: inputs)`.
fn staged(m: &mut Mutation<'_>, inputs: Vec<Value>) -> (Vec<Value>, Vec<Value>) {
    let mut request = Request::new(
        "mutation($input: [StagedUploadInput!]!) { stagedUploadsCreate(input: $input) { stagedTargets { url resourceUrl parameters { name value } } userErrors { field message } } }",
    );
    request
        .variables
        .insert("input".into(), Value::Array(inputs));
    let schema = m.schema();
    let answer = {
        let mut resolver = m.resolver();
        graphql::run(schema, &mut resolver, &request, None).to_json()
    };
    let payload = &answer["data"]["stagedUploadsCreate"];
    let list = |name: &str| payload[name].as_array().cloned().unwrap_or_default();
    (list("stagedTargets"), list("userErrors"))
}

fn input_of(value: &Value) -> Value {
    let mut input: Map<String, Value> = value.as_object().cloned().unwrap_or_default();
    input.retain(|key, _| {
        matches!(
            key.as_str(),
            "filename" | "mimeType" | "resource" | "httpMethod" | "fileSize"
        )
    });
    Value::Object(input)
}

/// `stagedUploadTargetGenerate(input:)`.
fn target_generate(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = input_of(ctx.arg("input").unwrap_or(&Value::Null));
    let (targets, errors) = staged(m, vec![input]);
    let errors = reindexed(errors, false);
    Ok(match targets.first() {
        Some(target) if errors.is_empty() => json!({
            "url": target["url"],
            "parameters": target["parameters"],
            "userErrors": [],
        }),
        _ => json!({"url": "", "parameters": [], "userErrors": errors}),
    })
}

/// `stagedUploadTargetsGenerate(input:)`.
fn targets_generate(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let inputs: Vec<Value> = ctx
        .arg("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(input_of)
        .collect();
    let (targets, errors) = staged(m, inputs);
    let errors = reindexed(errors, true);
    if !errors.is_empty() {
        return Ok(json!({"urls": null, "userErrors": errors}));
    }
    let urls: Vec<Value> = targets
        .iter()
        .map(|target| json!({"url": target["url"], "parameters": target["parameters"]}))
        .collect();
    Ok(json!({"urls": urls, "userErrors": []}))
}

/// The user errors of `stagedUploadsCreate` at the deprecated input's paths: `["input", "0",
/// "mimeType"]` stays for a list, loses its index for one input.
fn reindexed(errors: Vec<Value>, list: bool) -> Vec<Value> {
    errors
        .into_iter()
        .map(|mut error| {
            if !list
                && let Some(field) = error["field"].as_array_mut()
                && field.len() > 1
            {
                field.remove(1);
            }
            error
        })
        .collect()
}
