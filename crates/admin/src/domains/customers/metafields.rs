//! The `metafields` input of `customerCreate` and `customerUpdate` (`[MetafieldInput!]`): the
//! metafields of the customer, created or updated in place, as `metafieldsSet` would set them.
//!
//! The records are those the seed makes (`content::seed_metafields`): the id
//! `metafield_gid(owner, namespace, key)`, the namespace as Admin writes it (`$app` is the app's
//! reserved `app--<id>`), `value`, `jsonValue`, `reference(s)` of a reference type, and the
//! definition `content::attach_definition` finds. A metafield without a namespace is in the
//! app's (`$app`), as `metafieldsSet` puts it *(chosen: the input says the namespace is required
//! when creating, without saying what happens otherwise)*.

use serde_json::{Map, Value, json};

use crate::domains::content::{admin_namespace, attach_definition, json_value, metafield_gid};
use crate::resolve::UserError;
use crate::state::{ShopState, parse_gid, value};

/// A metafield input, read.
struct Input {
    /// The metafield it updates: its `id`, or the owner's of the same namespace and key.
    existing: Option<String>,
    namespace: String,
    key: String,
    kind: Option<String>,
    value: Option<String>,
}

/// The namespace as Admin writes it: `$app` and `$app:x` are the app's reserved ones.
fn namespace_of(state: &ShopState, namespace: &str) -> String {
    let app = state
        .keys("App")
        .next()
        .and_then(parse_gid)
        .map_or(0, |(_, id)| id);
    admin_namespace(app, namespace)
}

/// The metafield of `owner` with `namespace` and `key`.
fn find(state: &ShopState, owner: &str, namespace: &str, key: &str) -> Option<String> {
    state
        .get(owner)?
        .get("metafields")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find(|id| {
            state.get(id).is_some_and(|metafield| {
                metafield.get("namespace").and_then(Value::as_str) == Some(namespace)
                    && metafield.get("key").and_then(Value::as_str) == Some(key)
            })
        })
        .map(str::to_string)
}

/// The inputs read, with what is wrong with them (`field` paths under `metafields`). `owner`
/// is `None` for a resource being created, which has none yet.
fn read(
    state: &ShopState,
    owner: Option<&str>,
    inputs: &[Value],
    errors: &mut Vec<UserError>,
) -> Vec<Input> {
    let mut read = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let position = index.to_string();
        let field = |name: &str| input.get(name).and_then(Value::as_str).map(str::trim);
        let (existing, namespace, key) = if let Some(id) = field("id") {
            let found = state.get(id).filter(|metafield| {
                owner.is_some()
                    && metafield.get("owner").and_then(Value::as_str) == owner
                    && state.type_of(id) == Some("Metafield")
            });
            let Some(found) = found else {
                // Not published: lsf's words.
                errors.push(UserError::new(
                    &["metafields", &position, "id"],
                    "Metafield does not exist",
                ));
                continue;
            };
            let text = |name: &str| {
                found
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            (Some(id.to_string()), text("namespace"), text("key"))
        } else {
            let namespace = namespace_of(state, field("namespace").unwrap_or("$app"));
            let key = field("key").unwrap_or_default().to_string();
            // `key`: "Must be 2-64 characters long" (the input's description); Rails' messages.
            let length = key.chars().count();
            let too = if length < 2 {
                Some("Key is too short (minimum is 2 characters)")
            } else if length > 64 {
                Some("Key is too long (maximum is 64 characters)")
            } else {
                None
            };
            if let Some(message) = too {
                errors.push(UserError::new(&["metafields", &position, "key"], message));
            }
            let existing = owner.and_then(|owner| find(state, owner, &namespace, &key));
            (existing, namespace, key)
        };
        let kind = field("type").map(str::to_string);
        let value = field("value").map(str::to_string);
        if existing.is_none() && kind.is_none() {
            errors.push(UserError::new(
                &["metafields", &position, "type"],
                "Type can't be blank",
            ));
        }
        if value.is_none() {
            errors.push(UserError::new(
                &["metafields", &position, "value"],
                "Value can't be blank",
            ));
        }
        read.push(Input {
            existing,
            namespace,
            key,
            kind,
            value,
        });
    }
    read
}

/// Checks the metafield inputs of a mutation on `owner` (`None`: being created).
pub fn check(
    state: &ShopState,
    owner: Option<&str>,
    inputs: Option<&Value>,
    errors: &mut Vec<UserError>,
) {
    let inputs = inputs
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    read(state, owner, &inputs, errors);
}

/// Sets the metafields of `owner` (of `owner_type`: `CUSTOMER`) the inputs give, which
/// [`check`] found right. Returns whether one changed.
pub fn apply(state: &mut ShopState, owner: &str, owner_type: &str, inputs: Option<&Value>) -> bool {
    let inputs = inputs
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut ignored = Vec::new();
    let read = read(state, Some(owner), &inputs, &mut ignored);
    let now = state.timestamp();
    let mut changed = false;
    for input in read {
        let Some(text) = input.value else { continue };
        let id = input
            .existing
            .clone()
            .unwrap_or_else(|| metafield_gid(owner, &input.namespace, &input.key));
        let kind = input
            .kind
            .or_else(|| {
                state
                    .get(&id)
                    .and_then(|metafield| metafield.get("type"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "single_line_text_field".to_string());
        let created = state
            .get(&id)
            .and_then(|metafield| metafield.get("createdAt").cloned())
            .unwrap_or_else(|| json!(now));
        let mut record = value::object(json!({
            "__typename": "Metafield",
            "id": id,
            "namespace": input.namespace,
            "key": input.key,
            "type": kind,
            "value": text,
            "jsonValue": json_value(&kind, &text),
            "owner": owner,
            "ownerType": owner_type,
            "definition": null,
            "sizeInBytes": text.len(),
            "createdAt": created,
            "updatedAt": now,
        }));
        if kind.ends_with("_reference") {
            if kind.starts_with("list.") {
                let references: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
                record.insert("references".into(), json!(references));
            } else {
                record.insert("reference".into(), json!(text));
            }
        }
        state.insert(record);
        if let Some(Value::Array(owned)) = state
            .get_mut(owner)
            .and_then(|record| record.get_mut("metafields"))
            && !owned.iter().any(|item| item.as_str() == Some(id.as_str()))
        {
            owned.push(json!(id));
        }
        attach_definition(state, &id);
        changed = true;
    }
    changed
}

/// Removes the metafields of a record being deleted.
pub fn remove_all(state: &mut ShopState, owner: &Map<String, Value>) {
    // Through the content family, so that their definitions no longer list them.
    for id in owner
        .get("metafields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        crate::domains::content::metafields::remove(state, id);
    }
}
