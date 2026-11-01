//! The deprecated staged upload mutations, which run `stagedUploadsCreate` ("Use
//! `stagedUploadsCreate` instead."): `stagedUploadTargetGenerate` (one target: its `url` and
//! `parameters` at the top of the payload) and `stagedUploadTargetsGenerate` (`urls`, each a
//! `StagedUploadTarget`: `url` and `parameters`, no `resourceUrl`). The targets are those of
//! `stagedUploadsCreate`, on lsf (see [`super::uploads`]); its errors keep their messages, at the
//! path of the deprecated input (`["input", "mimeType"]` for the one target).

use serde_json::{Map, Value, json};

use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, mutation, payload};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("stagedUploadTargetGenerate", target_generate)
        .mutation("stagedUploadTargetsGenerate", targets_generate);
}

/// Runs `stagedUploadsCreate` with `input` (a list of `StagedUploadInput`s).
fn staged_uploads(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    input: Value,
) -> Result<Value, FieldError> {
    let Some(run) = m.registry().mutation_fn("stagedUploadsCreate") else {
        return Err(mutation::not_simulated("stagedUploadsCreate"));
    };
    let mut arguments = Map::new();
    arguments.insert("input".into(), input);
    run(m, &ctx.with_arguments(&arguments))
}

/// The user errors of a `stagedUploadsCreate` payload.
fn errors_of(answer: &Value) -> Vec<Value> {
    answer
        .get("userErrors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn target_generate(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg("input").cloned().unwrap_or(Value::Null);
    let answer = staged_uploads(m, ctx, json!([input]))?;
    let mut errors = errors_of(&answer);
    // `["input", "0", "mimeType"]` is `["input", "mimeType"]` here.
    for error in &mut errors {
        if let Some(Value::Array(field)) = error.get_mut("field")
            && field.get(1) == Some(&json!("0"))
        {
            field.remove(1);
        }
    }
    if !errors.is_empty() {
        let mut empty = payload(json!({"url": null, "parameters": null}), &[]);
        empty["userErrors"] = json!(errors);
        return Ok(empty);
    }
    let target = answer
        .get("stagedTargets")
        .and_then(|targets| targets.get(0))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(payload(
        json!({"url": target.get("url"), "parameters": target.get("parameters")}),
        &[],
    ))
}

fn targets_generate(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg("input").cloned().unwrap_or_else(|| json!([]));
    let answer = staged_uploads(m, ctx, input)?;
    let errors = errors_of(&answer);
    if !errors.is_empty() {
        let mut empty = payload(json!({"urls": null}), &[]);
        empty["userErrors"] = json!(errors);
        return Ok(empty);
    }
    let urls: Vec<Value> = answer
        .get("stagedTargets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|target| json!({"url": target.get("url"), "parameters": target.get("parameters")}))
        .collect();
    Ok(payload(json!({"urls": urls}), &[]))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domains::catalog::mutation_tests::{demo, ok, run};

    #[test]
    fn the_deprecated_staged_upload_mutations_answer_staged_upload_targets() {
        let mut shop = demo();
        let one = run(
            &mut shop,
            r#"mutation { stagedUploadTargetGenerate(input: {resource: FILE, filename: "notes.pdf", mimeType: "application/pdf", httpMethod: POST}) {
                url parameters { name value } userErrors { field message } } }"#,
        );
        let payload = ok(&one, "stagedUploadTargetGenerate");
        assert!(
            payload["url"]
                .as_str()
                .unwrap()
                .contains("/__lsf/admin/uploads/"),
            "{one}"
        );
        assert!(
            payload["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .any(|parameter| parameter["name"] == json!("key"))
        );
        let many = run(
            &mut shop,
            r#"mutation { stagedUploadTargetsGenerate(input: [{resource: IMAGE, filename: "a.png", mimeType: "image/png"},
                {resource: IMAGE, filename: "b.jpg", mimeType: "image/jpeg"}]) { urls { url parameters { name } } userErrors { message } } }"#,
        );
        let urls = ok(&many, "stagedUploadTargetsGenerate")["urls"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(urls.len(), 2);
        assert!(urls[1]["url"].as_str().unwrap().contains("b.jpg"));
    }

    #[test]
    fn a_deprecated_staged_upload_refuses_a_file_its_resource_does_not_take() {
        let mut shop = demo();
        let before = shop.clone();
        let refused = run(
            &mut shop,
            r#"mutation { stagedUploadTargetGenerate(input: {resource: IMAGE, filename: "notes.pdf", mimeType: "application/pdf"}) {
                url parameters { name } userErrors { field message } } }"#,
        );
        assert_eq!(
            refused["data"]["stagedUploadTargetGenerate"],
            // `url` and `parameters` are non-null: the empty values of a payload that failed.
            json!({"url": "", "parameters": [], "userErrors": [
                {"field": ["input", "mimeType"], "message": "notes.pdf: (application/pdf) is not a recognized format"}]})
        );
        assert!(shop.events().is_empty());
        assert_eq!(shop.len(), before.len());
    }
}
