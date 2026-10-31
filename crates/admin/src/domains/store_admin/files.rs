//! `fileAcknowledgeUpdateFailed`: "Acknowledges file update failure by resetting FAILED status
//! to READY and clearing any media errors."
//!
//! lsf's files are `READY` as soon as they are created (the content family's `files`), so a
//! file is `FAILED` only when something set it so; the mutation still does what Shopify's does
//! for the files that are: `fileStatus` (and a medium's `status`) back to `READY`, `fileErrors`
//! and `mediaErrors` emptied. A file that is not `FAILED` is refused (`NON_READY_STATE`, lsf's
//! wording), an id that is no file `FILE_DOES_NOT_EXIST`; nothing changes then.

use serde_json::{Value, json};

use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};

/// The types a `File` is.
const FILE_TYPES: [&str; 5] = [
    "ExternalVideo",
    "GenericFile",
    "MediaImage",
    "Model3d",
    "Video",
];

pub fn register(registry: &mut Registry) {
    registry.mutation("fileAcknowledgeUpdateFailed", acknowledge_update_failed);
}

fn acknowledge_update_failed(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let ids: Vec<String> = ctx
        .arg("fileIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let mut errors = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let position = index.to_string();
        let Some(record) = m
            .state()
            .type_of(id)
            .filter(|kind| FILE_TYPES.contains(kind))
            .and_then(|_| m.state().get(id))
        else {
            errors.push(
                UserError::new(
                    &["fileIds", &position],
                    format!("File id {id} does not exist."),
                )
                .code("FILE_DOES_NOT_EXIST"),
            );
            continue;
        };
        if record.get("fileStatus").and_then(Value::as_str) != Some("FAILED") {
            errors.push(
                UserError::new(
                    &["fileIds", &position],
                    format!("File {id} is not in a failed state."),
                )
                .code("NON_READY_STATE"),
            );
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"files": null}), &errors));
    }
    let state = m.state_mut();
    for id in &ids {
        if let Some(file) = state.get_mut(id) {
            file.insert("fileStatus".into(), json!("READY"));
            file.insert("fileErrors".into(), json!([]));
            if file.contains_key("status") {
                file.insert("status".into(), json!("READY"));
            }
            if file.contains_key("mediaErrors") {
                file.insert("mediaErrors".into(), json!([]));
            }
        }
        state.touch(id);
        state.emit("files/update", id);
    }
    Ok(payload(json!({"files": ids}), &[]))
}
