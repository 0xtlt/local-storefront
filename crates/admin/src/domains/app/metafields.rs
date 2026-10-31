//! The `metafields: [MetafieldInput!]` of the app family's inputs: the configuration a Function
//! reads, set on the discount, cart transform, customization or validation that runs it.
//! (`metafieldsSet`, the content family's, sets the metafields of any owner.)
//!
//! A metafield is created with `namespace`, `key`, `type` and `value`, or updated by `id` (or
//! by `namespace` and `key`) with a new `value`. A namespace not given is the app's reserved one
//! (`app--<app id>`), as `$app` is; `$app:config` is `app--<app id>--config`. The records are in
//! the shape the seed gives the metafields of `checkout.discounts` and `checkout.functions`, so
//! that the store sync writes them back as the settings of the Function.
//!
//! The checks are those whose failure would leave a Function without its configuration: the
//! fields a new metafield needs, and a `json`, `number_integer`, `number_decimal` or `boolean`
//! value that is not one. Their wording is lsf's (Rails' "can't be blank" for the missing
//! fields): Shopify's for these inputs is not published.

use serde_json::{Value, json};

use crate::domains::content::{admin_namespace, attach_definition, json_value, metafield_gid};
use crate::resolve::UserError;
use crate::state::{ShopState, parse_gid, value};

/// The numeric id of the app of the shop: what its reserved namespace names.
fn app_id(state: &ShopState) -> u64 {
    state
        .keys("App")
        .next()
        .and_then(parse_gid)
        .map_or(0, |(_, id)| id)
}

fn text<'v>(input: &'v Value, field: &str) -> Option<&'v str> {
    input.get(field).and_then(Value::as_str)
}

/// The metafield an input names among those of `owner`: by `id`, else by namespace and key.
fn existing(state: &ShopState, owner: Option<&str>, input: &Value) -> Option<String> {
    let owned: Vec<String> = owner
        .and_then(|owner| state.get(owner))
        .and_then(|record| record.get("metafields"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if let Some(id) = text(input, "id") {
        return owned.into_iter().find(|known| known == id);
    }
    let namespace = admin_namespace(app_id(state), text(input, "namespace").unwrap_or("$app"));
    let key = text(input, "key")?;
    owned.into_iter().find(|gid| {
        state.get(gid).is_some_and(|metafield| {
            metafield.get("namespace").and_then(Value::as_str) == Some(namespace.as_str())
                && metafield.get("key").and_then(Value::as_str) == Some(key)
        })
    })
}

/// Why a value does not fit its type, `None` when it does.
fn invalid_value(kind: &str, value: &str) -> Option<String> {
    match kind {
        "json" => serde_json::from_str::<Value>(value)
            .err()
            .map(|error| format!("Value is invalid JSON: {error}.")),
        "number_integer" => value
            .trim()
            .parse::<i64>()
            .is_err()
            .then(|| "Value must be an integer.".to_string()),
        "number_decimal" => value
            .trim()
            .parse::<f64>()
            .is_err()
            .then(|| "Value must be a decimal number.".to_string()),
        "boolean" => {
            (!matches!(value, "true" | "false")).then(|| "Value must be true or false.".to_string())
        }
        kind if kind.starts_with("list.") => serde_json::from_str::<Vec<Value>>(value)
            .err()
            .map(|_| "Value must be a JSON array.".to_string()),
        _ => None,
    }
}

/// Checks the metafield inputs of an input at `path` (`["automaticAppDiscount"]`): their errors,
/// with `code` (the payload's code for metafields, when its error type has one).
pub fn check(
    state: &ShopState,
    owner: Option<&str>,
    inputs: &[Value],
    path: &[&str],
    code: Option<&str>,
    errors: &mut Vec<UserError>,
) {
    for (index, input) in inputs.iter().enumerate() {
        let position = index.to_string();
        let at = |field: &str| -> Vec<String> {
            path.iter()
                .map(|part| part.to_string())
                .chain([
                    "metafields".to_string(),
                    position.clone(),
                    field.to_string(),
                ])
                .collect()
        };
        let mut push = |field: &str, message: String| {
            let path = at(field);
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            let mut error = UserError::new(&path, message);
            if let Some(code) = code {
                error = error.code(code);
            }
            errors.push(error);
        };
        let known = existing(state, owner, input);
        if text(input, "id").is_some() && known.is_none() {
            push("id", "Metafield does not exist".to_string());
            continue;
        }
        let stored_type = known
            .as_deref()
            .and_then(|gid| state.get(gid))
            .and_then(|metafield| metafield.get("type"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if known.is_none() {
            if text(input, "key").is_none_or(|key| key.trim().is_empty()) {
                push("key", "Key can't be blank".to_string());
            }
            if text(input, "type").is_none_or(|kind| kind.trim().is_empty()) {
                push("type", "Type can't be blank".to_string());
            }
        }
        let Some(value) = text(input, "value") else {
            push("value", "Value can't be blank".to_string());
            continue;
        };
        let kind = text(input, "type").map(str::to_string).or(stored_type);
        if let Some(message) = kind.and_then(|kind| invalid_value(&kind, value)) {
            push("value", message);
        }
    }
}

/// Creates or updates the metafields of `owner` from checked inputs, and keeps its `metafields`
/// list (the GIDs, in creation order). Answers whether anything changed.
pub fn apply(state: &mut ShopState, owner: &str, owner_type: &str, inputs: &[Value]) -> bool {
    let now = state.timestamp();
    let app = app_id(state);
    let mut changed = false;
    for input in inputs {
        let known = existing(state, Some(owner), input);
        let previous = known.as_deref().and_then(|gid| state.get(gid)).cloned();
        let field = |name: &str| {
            previous
                .as_ref()
                .and_then(|record| record.get(name))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let namespace = field("namespace")
            .unwrap_or_else(|| admin_namespace(app, text(input, "namespace").unwrap_or("$app")));
        let Some(key) = field("key").or_else(|| text(input, "key").map(str::to_string)) else {
            continue;
        };
        let Some(kind) = text(input, "type")
            .map(str::to_string)
            .or_else(|| field("type"))
        else {
            continue;
        };
        let value = text(input, "value").unwrap_or_default().to_string();
        let gid = known.unwrap_or_else(|| metafield_gid(owner, &namespace, &key));
        let created = field("createdAt").unwrap_or_else(|| now.clone());
        let mut record = value::object(json!({
            "__typename": "Metafield",
            "id": gid,
            "namespace": namespace,
            "key": key,
            "type": kind,
            "value": value,
            "jsonValue": json_value(&kind, &value),
            "owner": owner,
            "ownerType": owner_type,
            "definition": null,
            "sizeInBytes": value.len(),
            "createdAt": created,
            "updatedAt": now,
        }));
        if kind.ends_with("_reference") {
            if kind.starts_with("list.") {
                let references: Vec<Value> = serde_json::from_str(&value).unwrap_or_default();
                record.insert("references".into(), Value::Array(references));
            } else {
                record.insert("reference".into(), json!(value));
            }
        }
        let same = previous.as_ref().is_some_and(|previous| {
            previous.get("value") == record.get("value")
                && previous.get("type") == record.get("type")
        });
        if same {
            continue;
        }
        state.insert(record);
        attach_definition(state, &gid);
        if let Some(Value::Array(owned)) = state
            .get_mut(owner)
            .and_then(|record| record.get_mut("metafields"))
            && !owned
                .iter()
                .any(|known| known.as_str() == Some(gid.as_str()))
        {
            owned.push(json!(gid));
        }
        changed = true;
    }
    changed
}

/// Removes the metafields of an owner that is being deleted: they go with it, as on Shopify.
pub fn remove_all(state: &mut ShopState, owner: &str) {
    let owned: Vec<String> = state
        .get(owner)
        .and_then(|record| record.get("metafields"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    for gid in owned {
        let definition = state
            .get(&gid)
            .and_then(|metafield| metafield.get("definition"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(definition) = definition.and_then(|id| state.get_mut(&id))
            && let Some(Value::Array(metafields)) = definition.get_mut("metafields")
        {
            metafields.retain(|known| known.as_str() != Some(gid.as_str()));
            let count = metafields.len();
            definition.insert("metafieldsCount".into(), json!(count));
        }
        state.remove(&gid);
    }
}

/// The metafield inputs of an input object (`metafields`), empty when there are none.
pub fn inputs_of(input: &serde_json::Map<String, Value>) -> Vec<Value> {
    input
        .get("metafields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
