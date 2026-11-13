//! Metaobjects and their definitions: `metaobjectDefinitionCreate`, `metaobjectDefinitionUpdate`,
//! `metaobjectDefinitionDelete`, `metaobjectCreate`, `metaobjectUpdate`, `metaobjectUpsert`,
//! `metaobjectDelete`, `metaobjectBulkDelete`; the `metaobjects(type:)`, `metaobjectByHandle`
//! and `metaobjectDefinitionByType` queries; `Metaobject.field(key:)` and `Metaobject.values`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A type prefixed `$app:` is the app's (`app--<app id>--<type>`), in every argument that takes
//!   a type. "The type must match a definition that already exists in the shop."
//! - `handle` "is auto-generated when omitted": lsf makes it from the display name, else the
//!   type, with `-1`, `-2` when it is taken (within the type).
//! - Field values are checked against the field definitions as metafield values are
//!   ([`super::types`]); a required field must have one. `fields` patches (the other fields keep
//!   their value), `values` "is a full replacement — omitted keys are cleared"; the two cannot be
//!   used together.
//! - With the `publishable` capability, an entry created without a status is a `DRAFT` (it is
//!   not on the storefront until it is `ACTIVE`).
//! - `Metaobject.values` is a JSON object of every field of the definition, by key in
//!   alphabetical order, `null` for a field without a value (the examples of `metaobjectCreate`
//!   and `metaobjectUpsert`); `fields` lists every field of the definition, in its order.
//! - Deleting a definition "also deletes all related metafield definitions, metaobjects, and
//!   metafields"; `metaobjectBulkDelete` deletes at once and answers a `Job` that is `done`.
//!
//! Ids: an entry created gets `metaobject_gid(type, handle)`, the id lsf-core gives the
//! metaobject of the store data (store sync writes it back with that id), else a new one when
//! another entry holds it (one renamed since). A definition gets `metaobject_definition_gid`.
//! Both are made of the type as the store data writes it (`$app:author` for the app's
//! `app--<id>--author`: [`super::data_type`]).
//!
//! Messages: Shopify does not publish them; lsf's say what the code's description says
//! ("No metaobject definition exists for type \"author\"").
//!
//! Events: `metaobjects/create`, `metaobjects/update`, `metaobjects/delete`. Shopify has no topic
//! for metaobject definitions: their changes are the events `metaobject_definitions/<action>`,
//! which no subscription can name.

use serde_json::{Map, Value, json};

use super::metafield_definitions::access_of;
use super::metafields::{shop_currency, text};
use super::types::{self, Context};
use super::{data_type, metaobject_definition_gid, metaobject_gid};
use crate::admin::domains::catalog::gids;
use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::admin::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .query("metaobjects", metaobjects)
        .query("metaobjectByHandle", metaobject_by_handle)
        .query("metaobjectDefinitionByType", definition_by_type)
        .field("Metaobject", "field", metaobject_field)
        .field("Metaobject", "values", metaobject_values)
        .field("Metaobject", "thumbnailField", |_, _, _| Ok(Output::Null))
        .field("Metaobject", "referencedBy", referenced_by)
        .default_text("Metaobject", |_r, record| {
            ["displayName", "handle"]
                .iter()
                .filter_map(|field| record.get(*field)?.as_str().map(str::to_string))
                .collect()
        })
        .mutation("metaobjectDefinitionCreate", definition_create)
        .mutation("metaobjectDefinitionUpdate", definition_update)
        .mutation("metaobjectDefinitionDelete", definition_delete)
        .mutation("metaobjectCreate", create)
        .mutation("metaobjectUpdate", update)
        .mutation("metaobjectUpsert", upsert)
        .mutation("metaobjectDelete", delete)
        .mutation("metaobjectBulkDelete", bulk_delete);
}

/// A type as the Admin API stores it: `$app:author` is `app--<app id>--author`.
pub fn stored_type(state: &ShopState, kind: &str) -> String {
    match kind.strip_prefix("$app:") {
        Some(name) => format!("app--{}--{name}", state.app_id()),
        None => kind.to_string(),
    }
}

/// The definition of a type (as stored).
pub fn definition_of(state: &ShopState, kind: &str) -> Option<String> {
    state
        .keys("MetaobjectDefinition")
        .find(|id| {
            state
                .get(id)
                .is_some_and(|definition| text(definition, "type") == kind)
        })
        .map(str::to_string)
}

/// The handle of the web pages of a type (`capabilities.onlineStore.data.urlHandle` of its
/// definition), else the type: its entries are at `/pages/<url handle>/<handle>`.
pub fn url_handle<'a>(state: &'a ShopState, kind: &'a str) -> &'a str {
    definition_of(state, kind)
        .and_then(|id| state.get(&id))
        .and_then(|definition| {
            definition
                .get("capabilities")?
                .pointer("/onlineStore/data/urlHandle")?
                .as_str()
        })
        .unwrap_or(kind)
}

fn entry_of(state: &ShopState, kind: &str, handle: &str) -> Option<String> {
    state
        .with_handle("Metaobject", handle, Some(("type", kind)))
        .first()
        .map(|id| id.to_string())
}

// ---- queries -----------------------------------------------------------------------------

/// `metaobjects(type:)`: the entries of one type, `$app:` read as the app's.
fn metaobjects(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let kind = stored_type(r.state(), ctx.arg_str("type").unwrap_or_default());
    let items: Vec<Value> = r
        .records_of("Metaobject")
        .into_iter()
        .filter(|item| {
            r.item_map(item)
                .is_some_and(|entry| text(entry, "type") == kind)
        })
        .collect();
    r.connection(ctx, items)
}

/// `metaobjectByHandle(handle: {type, handle})`.
fn metaobject_by_handle(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let handle = ctx.arg("handle");
    let get = |name: &str| {
        handle
            .and_then(|handle| handle.get(name))
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let kind = stored_type(r.state(), get("type"));
    Ok(Output::object_or_null(
        entry_of(r.state(), &kind, get("handle")).map(|id| Obj::record(&id)),
    ))
}

/// `metaobjectDefinitionByType(type:)`.
fn definition_by_type(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let kind = stored_type(r.state(), ctx.arg_str("type").unwrap_or_default());
    Ok(Output::object_or_null(
        definition_of(r.state(), &kind).map(|id| Obj::record(&id)),
    ))
}

/// `Metaobject.field(key:)`.
fn metaobject_field(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let key = ctx.arg_str("key").unwrap_or_default();
    let field = r
        .items_of(object, "fields")
        .into_iter()
        .find(|field| field.get("key").and_then(Value::as_str) == Some(key));
    match field {
        Some(Value::Object(field)) => Ok(Output::Object(Obj::inline("MetaobjectField", field))),
        _ => Ok(Output::Null),
    }
}

/// `Metaobject.referencedBy`: the metafields and the fields of other metaobjects that reference
/// the entry (a `metaobject_reference`, a `mixed_reference`, or a list of them), as
/// `MetafieldRelation`s: `key` and `namespace` of the metafield, or the field's key and the
/// type of the metaobject that has it ("The namespace of the metafield making the reference,
/// or type of the metaobject"); `name` the name of the definition, else the key; `referencer`
/// the owner of the metafield, or the metaobject; `target` the entry. Metafields first, in
/// creation order.
fn referenced_by(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(target) = object.key().map(str::to_string) else {
        return r.connection(ctx, Vec::new());
    };
    let state = r.state();
    let names = |value: &Value| -> bool {
        value.get("reference").and_then(Value::as_str) == Some(target.as_str())
            || value
                .get("references")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(&target)))
    };
    let mut items = Vec::new();
    for id in state.keys("Metafield") {
        let Some(metafield) = state.get(id) else {
            continue;
        };
        if !names(&Value::Object(metafield.clone())) {
            continue;
        }
        let key = text(metafield, "key");
        let name = metafield
            .get("definition")
            .and_then(Value::as_str)
            .and_then(|definition| state.get(definition))
            .map(|definition| text(definition, "name"))
            .filter(|name| !name.is_empty())
            .unwrap_or(key);
        items.push(json!({
            "__typename": "MetafieldRelation",
            "key": key,
            "namespace": text(metafield, "namespace"),
            "name": name,
            "referencer": metafield.get("owner"),
            "target": target,
        }));
    }
    for id in state.keys("Metaobject") {
        let Some(entry) = state.get(id) else {
            continue;
        };
        for field in entry
            .get("fields")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|field| names(field))
        {
            let key = field.get("key").and_then(Value::as_str).unwrap_or_default();
            let name = field
                .pointer("/definition/name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or(key);
            items.push(json!({
                "__typename": "MetafieldRelation",
                "key": key,
                "namespace": text(entry, "type"),
                "name": name,
                "referencer": id,
                "target": target,
            }));
        }
    }
    r.connection(ctx, items)
}

/// `Metaobject.values`: every field of the definition by key, in alphabetical order, its JSON
/// value or `null`.
fn metaobject_values(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(record) = r.map_of(object) else {
        return Ok(Output::Null);
    };
    Ok(Output::value(values_of(r.state(), record)))
}

pub fn values_of(state: &ShopState, record: &Record) -> Value {
    let mut keys: Vec<String> = record
        .get("definition")
        .and_then(Value::as_str)
        .and_then(|definition| state.get(definition))
        .and_then(|definition| definition.get("fieldDefinitions"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|field| field.get("key")?.as_str().map(str::to_string))
        .collect();
    let fields: Vec<&Value> = record
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    for field in &fields {
        if let Some(key) = field.get("key").and_then(Value::as_str)
            && !keys.iter().any(|known| known == key)
        {
            keys.push(key.to_string());
        }
    }
    keys.sort();
    let values: Map<String, Value> = keys
        .into_iter()
        .map(|key| {
            let value = fields
                .iter()
                .find(|field| field.get("key").and_then(Value::as_str) == Some(key.as_str()))
                .and_then(|field| field.get("jsonValue"))
                .cloned()
                .unwrap_or(Value::Null);
            (key, value)
        })
        .collect();
    Value::Object(values)
}

// ---- errors ------------------------------------------------------------------------------

/// A `MetaobjectUserError`: a [`UserError`] and the key of the field it is about
/// (`elementKey`).
struct MetaobjectError {
    error: UserError,
    key: Option<String>,
}

impl MetaobjectError {
    fn new(field: &[&str], message: impl Into<String>, code: &str) -> MetaobjectError {
        MetaobjectError {
            error: UserError::new(field, message).code(code),
            key: None,
        }
    }

    fn key(mut self, key: &str) -> MetaobjectError {
        self.key = Some(key.to_string());
        self
    }
}

/// A payload with `MetaobjectUserError`s.
fn answer(fields: Value, errors: &[MetaobjectError]) -> Value {
    let plain: Vec<UserError> = errors
        .iter()
        .map(|error| {
            let mut plain = error.error.clone();
            plain.code = super::fit_code(plain.code.as_deref(), "MetaobjectUserErrorCode");
            plain
        })
        .collect();
    let mut answer = payload(fields, &plain);
    if let Some(Value::Array(list)) = answer.get_mut("userErrors") {
        for (entry, error) in list.iter_mut().zip(errors) {
            if let Some(key) = &error.key {
                entry["elementKey"] = json!(key);
            }
        }
    }
    answer
}

/// The checks of a type: 3 to 255 characters (after `$app:`), letters, digits, `-` and `_`.
fn check_type(kind: &str, field: &[&str], errors: &mut Vec<MetaobjectError>) {
    let name = kind.strip_prefix("$app:").unwrap_or(kind);
    let length = name.chars().count();
    if length < 3 {
        errors.push(MetaobjectError::new(
            field,
            "Type is too short (minimum is 3 characters)",
            "TOO_SHORT",
        ));
    } else if length > 255 {
        errors.push(MetaobjectError::new(
            field,
            "Type is too long (maximum is 255 characters)",
            "TOO_LONG",
        ));
    } else if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        errors.push(MetaobjectError::new(
            field,
            "Type contains one or more invalid characters.",
            "INVALID",
        ));
    }
}

// ---- definitions -------------------------------------------------------------------------

/// A `MetaobjectFieldDefinition` from its input, checked (`path` is the input's).
fn field_definition(
    state: &ShopState,
    input: &Map<String, Value>,
    path: &[&str],
    errors: &mut Vec<MetaobjectError>,
) -> Option<Value> {
    let get = |name: &str| input.get(name).and_then(Value::as_str);
    let key = get("key").unwrap_or_default();
    let kind = get("type").unwrap_or_default();
    let mut field = path.to_vec();
    field.push("key");
    let length = key.chars().count();
    if !(1..=64).contains(&length)
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        errors.push(MetaobjectError::new(&field, "Key is invalid", "INVALID").key(key));
        return None;
    }
    if !types::is_type(kind) {
        let mut field = path.to_vec();
        field.push("type");
        errors.push(
            MetaobjectError::new(
                &field,
                format!("Type '{kind}' is not a valid type."),
                "INVALID",
            )
            .key(key),
        );
        return None;
    }
    let mut validation_errors = Vec::new();
    let mut field = path.to_vec();
    field.push("validations");
    let validations = super::metafield_definitions::checked_validations(
        state,
        kind,
        input.get("validations"),
        &field,
        &mut validation_errors,
    );
    for error in validation_errors {
        errors.push(MetaobjectError {
            error,
            key: Some(key.to_string()),
        });
    }
    Some(json!({
        "key": key,
        "name": get("name").map_or_else(|| value::humanize(key), str::to_string),
        "description": get("description"),
        "required": input.get("required").and_then(Value::as_bool).unwrap_or(false),
        "type": types::definition_type(kind),
        "validations": validations,
        "capabilities": {"adminFilterable": {
            "eligible": true,
            "enabled": input
                .get("capabilities")
                .and_then(|capabilities| capabilities.pointer("/adminFilterable/enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }},
    }))
}

/// The capabilities of a definition: the input's, over what it had.
fn capabilities_of(input: Option<&Value>, before: Option<&Value>) -> Value {
    let enabled = |name: &str| {
        input
            .and_then(|capabilities| capabilities.get(name))
            .and_then(|capability| capability.get("enabled"))
            .and_then(Value::as_bool)
            .or_else(|| {
                before
                    .and_then(|capabilities| capabilities.get(name))
                    .and_then(|capability| capability.get("enabled"))
                    .and_then(Value::as_bool)
            })
            .unwrap_or(false)
    };
    let data = |name: &str| {
        input
            .and_then(|capabilities| capabilities.get(name))
            .and_then(|capability| capability.get("data"))
            .filter(|data| !data.is_null())
            .or_else(|| {
                before
                    .and_then(|capabilities| capabilities.get(name))
                    .and_then(|capability| capability.get("data"))
            })
            .cloned()
            .unwrap_or(Value::Null)
    };
    let online_data = match data("onlineStore") {
        Value::Object(data) => json!({
            "urlHandle": data.get("urlHandle").cloned().unwrap_or(Value::Null),
            "canCreateRedirects": data
                .get("canCreateRedirects")
                .or_else(|| data.get("createRedirects"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }),
        _ => Value::Null,
    };
    json!({
        "publishable": {"enabled": enabled("publishable")},
        "translatable": {"enabled": enabled("translatable")},
        "renderable": {"enabled": enabled("renderable"), "data": data("renderable")},
        "onlineStore": {"enabled": enabled("onlineStore"), "data": online_data},
    })
}

fn definition_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("definition");
    let get = |name: &str| input.get(name).and_then(Value::as_str);
    let state = mutation.state();
    let mut errors = Vec::new();
    let given_type = get("type").unwrap_or_default();
    check_type(given_type, &["definition", "type"], &mut errors);
    let kind = stored_type(state, given_type);
    if errors.is_empty() && definition_of(state, &kind).is_some() {
        errors.push(MetaobjectError::new(
            &["definition", "type"],
            "Type has already been taken",
            "TAKEN",
        ));
    }
    let mut fields = Vec::new();
    for (index, field) in input
        .get("fieldDefinitions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let index = index.to_string();
        let path = ["definition", "fieldDefinitions", index.as_str()];
        let Some(field) = field
            .as_object()
            .and_then(|field| field_definition(state, field, &path, &mut errors))
        else {
            continue;
        };
        let key = field.get("key").and_then(Value::as_str).unwrap_or_default();
        if fields
            .iter()
            .any(|known: &Value| known.get("key").and_then(Value::as_str) == Some(key))
        {
            errors.push(
                MetaobjectError::new(&path, "Field keys must be unique.", "DUPLICATE_FIELD_INPUT")
                    .key(key),
            );
            continue;
        }
        fields.push(field);
    }
    let display = get("displayNameKey");
    if let Some(display) = display
        && !fields
            .iter()
            .any(|field| field.get("key").and_then(Value::as_str) == Some(display))
    {
        errors.push(MetaobjectError::new(
            &["definition", "displayNameKey"],
            "Display name key must be the key of a field definition.",
            "INVALID",
        ));
    }
    if input
        .get("access")
        .and_then(|access| access.get("admin"))
        .is_some_and(|admin| !admin.is_null())
        && !kind.starts_with("app--")
    {
        errors.push(MetaobjectError::new(
            &["definition", "access", "admin"],
            "Admin access can only be specified on metaobject definitions that have an app-reserved type.",
            "ADMIN_ACCESS_INPUT_NOT_ALLOWED",
        ));
    }
    if !errors.is_empty() {
        return Ok(answer(json!({"metaobjectDefinition": null}), &errors));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let stable = metaobject_definition_gid(&data_type(state.app_id(), &kind));
    let id = state.created_gid("MetaobjectDefinition", stable);
    let app = state.app_gid().map(str::to_string);
    state.insert(value::object(json!({
        "__typename": "MetaobjectDefinition",
        "id": id,
        "type": kind,
        "name": get("name").map_or_else(|| value::humanize(given_type.trim_start_matches("$app:")), str::to_string),
        "description": get("description"),
        "displayNameKey": display,
        "fieldDefinitions": fields,
        "hasThumbnailField": false,
        "metaobjects": [],
        "metaobjectsCount": 0,
        "capabilities": capabilities_of(input.get("capabilities"), None),
        "access": access_of(input.get("access"), kind.starts_with("app--"), None),
        "createdByApp": app,
        "standardTemplate": null,
        "createdAt": now,
        "updatedAt": now,
    })));
    state.emit("metaobject_definitions/create", &id);
    Ok(answer(json!({"metaobjectDefinition": id}), &[]))
}

/// `metaobjectDefinitionUpdate`: name, description, display name key, capabilities, access, and
/// the field definitions (`create`, `update`, `delete`; `resetFieldOrder` puts them in the
/// order of the operations).
fn definition_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = ctx.arg_object_or_default("definition");
    let state = mutation.state();
    let Some(record) = state.get_of(&id, "MetaobjectDefinition").cloned() else {
        let error = MetaobjectError::new(&["id"], "Record not found", "RECORD_NOT_FOUND");
        return Ok(answer(json!({"metaobjectDefinition": null}), &[error]));
    };
    let mut errors = Vec::new();
    let mut fields: Vec<Value> = record
        .get("fieldDefinitions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut order: Vec<String> = Vec::new();
    for (index, operation) in input
        .get("fieldDefinitions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let index = index.to_string();
        if let Some(create) = operation.get("create").and_then(Value::as_object) {
            let path = ["definition", "fieldDefinitions", index.as_str(), "create"];
            if let Some(field) = field_definition(state, create, &path, &mut errors) {
                let key = field["key"].as_str().unwrap_or_default().to_string();
                if fields
                    .iter()
                    .any(|known| known.get("key").and_then(Value::as_str) == Some(&key))
                {
                    errors.push(
                        MetaobjectError::new(
                            &path,
                            "Field keys must be unique.",
                            "DUPLICATE_FIELD_INPUT",
                        )
                        .key(&key),
                    );
                } else {
                    order.push(key);
                    fields.push(field);
                }
            }
        } else if let Some(update) = operation.get("update").and_then(Value::as_object) {
            let path = ["definition", "fieldDefinitions", index.as_str(), "update"];
            let key = update
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(field) = fields
                .iter_mut()
                .find(|known| known.get("key").and_then(Value::as_str) == Some(key))
            else {
                errors.push(
                    MetaobjectError::new(
                        &path,
                        "No field definition found for this key.",
                        "UNDEFINED_OBJECT_FIELD",
                    )
                    .key(key),
                );
                continue;
            };
            order.push(key.to_string());
            for name in ["name", "description", "required"] {
                if let Some(value) = update.get(name).filter(|value| !value.is_null()) {
                    field[name] = value.clone();
                }
            }
            if update.contains_key("validations") {
                let kind = field
                    .pointer("/type/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mut path = path.to_vec();
                path.push("validations");
                let mut validation_errors = Vec::new();
                let validations = super::metafield_definitions::checked_validations(
                    state,
                    &kind,
                    update.get("validations"),
                    &path,
                    &mut validation_errors,
                );
                for error in validation_errors {
                    errors.push(MetaobjectError {
                        error,
                        key: Some(key.to_string()),
                    });
                }
                field["validations"] = json!(validations);
            }
        } else if let Some(delete) = operation.get("delete") {
            let key = delete
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let before = fields.len();
            fields.retain(|known| known.get("key").and_then(Value::as_str) != Some(key));
            if fields.len() == before {
                errors.push(
                    MetaobjectError::new(
                        &["definition", "fieldDefinitions", index.as_str(), "delete"],
                        "No field definition found for this key.",
                        "UNDEFINED_OBJECT_FIELD",
                    )
                    .key(key),
                );
            }
        }
    }
    if input.get("resetFieldOrder").and_then(Value::as_bool) == Some(true) {
        fields.sort_by_key(|field| {
            let key = field.get("key").and_then(Value::as_str).unwrap_or_default();
            order
                .iter()
                .position(|known| known == key)
                .unwrap_or(usize::MAX)
        });
    }
    let display = match input.get("displayNameKey") {
        Some(Value::String(key)) => Some(key.clone()),
        Some(_) => None,
        None => record
            .get("displayNameKey")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    if let Some(display) = &display
        && !fields
            .iter()
            .any(|field| field.get("key").and_then(Value::as_str) == Some(display.as_str()))
    {
        errors.push(MetaobjectError::new(
            &["definition", "displayNameKey"],
            "Display name key must be the key of a field definition.",
            "INVALID",
        ));
    }
    if !errors.is_empty() {
        return Ok(answer(json!({"metaobjectDefinition": null}), &errors));
    }
    let app_owned = text(&record, "type").starts_with("app--");
    let was_publishable = record
        .get("capabilities")
        .and_then(|capabilities| capabilities.pointer("/publishable/enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let capabilities = capabilities_of(input.get("capabilities"), record.get("capabilities"));
    let publishable = capabilities
        .pointer("/publishable/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let state = mutation.state_mut();
    if let Some(definition) = state.get_mut(&id) {
        for name in ["name", "description"] {
            if let Some(value) = input.get(name) {
                definition.insert(name.into(), value.clone());
            }
        }
        definition.insert("displayNameKey".into(), json!(display));
        definition.insert("fieldDefinitions".into(), json!(fields));
        definition.insert("capabilities".into(), capabilities);
        if input.contains_key("access") {
            let access = access_of(input.get("access"), app_owned, definition.get("access"));
            definition.insert("access".into(), access);
        }
    }
    state.touch(&id);
    // The entries follow: their fields, their display names, their status.
    let entries = entries_of(state, &id);
    for entry in entries {
        let changed = refresh_entry(
            state,
            &entry,
            &fields,
            display.as_deref(),
            publishable,
            was_publishable,
        );
        if changed {
            state.touch(&entry);
            state.emit("metaobjects/update", &entry);
        }
    }
    state.emit("metaobject_definitions/update", &id);
    Ok(answer(json!({"metaobjectDefinition": id}), &[]))
}

/// Where an uninstall keeps each app-owned metaobject definition it deleted, with its entries,
/// for a reinstall to create them again: a memo per definition, under this prefix and its type.
const UNINSTALLED: &str = "lsf/uninstalled-metaobject-definition:";

/// What uninstalling the app did to its metaobject definitions ([`remove_app_owned`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Removed {
    /// The definitions of its reserved types deleted.
    pub definitions: usize,
    /// Their entries, kept aside.
    pub metaobjects: usize,
}

/// What uninstalling the app does to its metaobject definitions: those of its reserved types
/// (`$app:<type>`, stored `app--<app id>--<type>`) are deleted, declared in the store data or made
/// with `metaobjectDefinitionCreate` alike, and their entries are kept aside with them (a memo per
/// definition, not records) for [`restore_app_owned`]. The definitions of other types are the
/// merchant's and stay, with their entries. No webhook: the app's subscriptions go with it (and
/// Shopify has no topic for metaobject definitions), so each removal emits `CHANGED`.
///
/// As shopify.dev says ("Manage metaobject definitions", "App uninstall and reinstall",
/// <https://shopify.dev/docs/apps/build/metaobjects/manage-metaobject-definitions#app-uninstall-and-reinstall>):
/// "When a user uninstalls your app, Shopify deletes its app-owned metaobject definitions and
/// temporarily retains the metaobjects and their values without a definition. This applies to any
/// app-owned definition, whether declared in `shopify.app.toml` or created with the GraphQL Admin
/// API." Shopify deletes the TOML ones during the uninstall and the others in the background
/// ("can persist after uninstall until asynchronous cleanup removes them"); lsf deletes both at
/// once. The entries kept are not records meanwhile: a `Metaobject` has a non-null `definition`,
/// and `metaobjects(type:)` takes the type of "a definition that already exists in the shop", so
/// no query reaches them, and the storefront does not render them. Answers what was removed.
pub fn remove_app_owned(state: &mut ShopState) -> Removed {
    let prefix = format!("app--{}--", state.app_id());
    let definitions: Vec<String> = state
        .keys("MetaobjectDefinition")
        .filter(|id| {
            state
                .get(id)
                .is_some_and(|definition| text(definition, "type").starts_with(&prefix))
        })
        .map(str::to_string)
        .collect();
    let mut removed = Removed::default();
    for id in &definitions {
        let Some(record) = state.get(id).cloned() else {
            continue;
        };
        let entries = entries_of(state, id);
        let kept: Vec<Value> = entries
            .iter()
            .filter_map(|entry| state.get(entry).cloned().map(Value::Object))
            .collect();
        let memo = format!("{UNINSTALLED}{}", text(&record, "type"));
        state.set_memo(
            memo,
            json!({"definition": Value::Object(record), "metaobjects": kept}),
        );
        for entry in &entries {
            state.emit(crate::admin::domains::app::CHANGED, entry);
            state.remove(entry);
        }
        state.emit(crate::admin::domains::app::CHANGED, id);
        state.remove(id);
        removed.definitions += 1;
        removed.metaobjects += entries.len();
    }
    removed
}

/// What installing the app again does to the metaobject definitions an uninstall deleted
/// ([`remove_app_owned`]): each is created again, under a new id, with the entries kept meanwhile
/// (their ids unchanged), which now name it. One made meanwhile of the same type stays as it is,
/// and the entries kept for the type are dropped. A metafield definition whose validation named
/// the old id names the new one. Answers how many definitions were created.
///
/// As shopify.dev says: "If the same app is reinstalled while that data is still retained, then
/// Shopify recreates the definitions and associates the existing metaobjects with them", and "The
/// new definitions have new GIDs, so a definition ID from before the uninstall no longer
/// resolves" ("Look up definitions by type instead"). Shopify keeps the entries for a while
/// ("Shopify doesn't guarantee how long"); lsf keeps them until the store data is seeded again.
pub fn restore_app_owned(state: &mut ShopState) -> usize {
    let mut kept: Vec<(String, Value)> = state
        .memos_with_prefix(UNINSTALLED)
        .map(|(memo, kept)| (memo.to_string(), kept.clone()))
        .collect();
    // In the order they were first created, as a listing by id shows them.
    kept.sort_by_key(|(memo, kept)| {
        let id = kept.pointer("/definition/id").and_then(Value::as_str);
        (id.map_or(0, crate::admin::state::gid_number), memo.clone())
    });
    let mut restored = 0;
    for (memo, kept) in kept {
        state.remove_memo(&memo);
        let Some(mut record) = kept.get("definition").and_then(Value::as_object).cloned() else {
            continue;
        };
        let kind = text(&record, "type").to_string();
        if definition_of(state, &kind).is_some() {
            continue;
        }
        let old = text(&record, "id").to_string();
        let id = state.new_gid("MetaobjectDefinition");
        let entries: Vec<Record> = kept
            .get("metaobjects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.as_object().cloned())
            .filter(|entry| !state.contains(text(entry, "id")))
            .collect();
        let ids: Vec<Value> = entries
            .iter()
            .map(|entry| json!(text(entry, "id")))
            .collect();
        record.insert("id".into(), json!(id));
        record.insert("metaobjectsCount".into(), json!(ids.len()));
        record.insert("metaobjects".into(), Value::Array(ids));
        state.insert(record);
        state.emit(crate::admin::domains::app::CHANGED, &id);
        for mut entry in entries {
            entry.insert("definition".into(), json!(id));
            let entry_id = text(&entry, "id").to_string();
            state.insert(entry);
            state.emit(crate::admin::domains::app::CHANGED, &entry_id);
        }
        rename_in_validations(state, &old, &id);
        restored += 1;
    }
    restored
}

/// The metafield definitions whose validations name the metaobject definition `old` name `new`.
fn rename_in_validations(state: &mut ShopState, old: &str, new: &str) {
    let naming: Vec<String> = state
        .keys("MetafieldDefinition")
        .filter(|definition| {
            state.get(definition).is_some_and(|definition| {
                definition
                    .get("validations")
                    .is_some_and(|validations| validations.to_string().contains(old))
            })
        })
        .map(str::to_string)
        .collect();
    for definition in naming {
        let Some(record) = state.get_mut(&definition) else {
            continue;
        };
        if let Some(Value::Array(validations)) = record.get_mut("validations") {
            for validation in validations {
                if let Some(Value::String(value)) = validation.get_mut("value")
                    && value.contains(old)
                {
                    *value = value.replace(old, new);
                }
            }
        }
        state.emit(crate::admin::domains::app::CHANGED, &definition);
    }
}

fn entries_of(state: &ShopState, definition: &str) -> Vec<String> {
    state
        .keys("Metaobject")
        .filter(|entry| {
            state
                .get(entry)
                .is_some_and(|entry| text(entry, "definition") == definition)
        })
        .map(str::to_string)
        .collect()
}

/// Brings an entry in line with its definition after an update: the fields removed are gone, the
/// field definitions it holds are the new ones, the display name follows the display name key,
/// and its status appears (`ACTIVE`) or goes with the publishable capability. Whether it changed.
fn refresh_entry(
    state: &mut ShopState,
    entry: &str,
    fields: &[Value],
    display: Option<&str>,
    publishable: bool,
    was_publishable: bool,
) -> bool {
    let Some(record) = state.get(entry).cloned() else {
        return false;
    };
    let mut updated = record.clone();
    let held: Vec<Value> = record
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let new_fields: Vec<Value> = fields
        .iter()
        .map(|definition| {
            let key = definition
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut field = held
                .iter()
                .find(|field| field.get("key").and_then(Value::as_str) == Some(key))
                .cloned()
                .unwrap_or_else(|| empty_field(definition));
            field["definition"] = definition.clone();
            field
        })
        .collect();
    updated.insert("fields".into(), json!(new_fields));
    updated.insert(
        "displayName".into(),
        json!(display_name(&new_fields, display, text(&record, "handle"))),
    );
    let mut capabilities = record
        .get("capabilities")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if publishable && !was_publishable {
        capabilities.insert("publishable".into(), json!({"status": "ACTIVE"}));
    } else if !publishable {
        capabilities.remove("publishable");
    }
    updated.insert("capabilities".into(), Value::Object(capabilities));
    if updated == record {
        return false;
    }
    if let Some(slot) = state.get_mut(entry) {
        *slot = updated;
    }
    true
}

/// `metaobjectDefinitionDelete`: the definition, its entries (`metaobjects/delete` each), and the
/// metafield definitions whose validations name it.
fn definition_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if mutation.state().type_of(&id) != Some("MetaobjectDefinition") {
        let error = MetaobjectError::new(&["id"], "Record not found", "RECORD_NOT_FOUND");
        return Ok(answer(json!({"deletedId": null}), &[error]));
    }
    let state = mutation.state_mut();
    let entries = entries_of(state, &id);
    for entry in entries {
        state.delete(&entry, "metaobjects/delete");
    }
    let referring: Vec<String> = state
        .keys("MetafieldDefinition")
        .filter(|definition| {
            state.get(definition).is_some_and(|definition| {
                definition
                    .get("validations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|validation| {
                        validation
                            .get("value")
                            .and_then(Value::as_str)
                            .is_some_and(|value| value.contains(id.as_str()))
                    })
            })
        })
        .map(str::to_string)
        .collect();
    for definition in referring {
        let metafields = gids(
            state
                .get(&definition)
                .and_then(|record| record.get("metafields")),
        );
        state.delete(&definition, "metafield_definitions/delete");
        for metafield in metafields {
            let owner = state
                .get(&metafield)
                .map(|record| text(record, "owner").to_string());
            super::metafields::remove(state, &metafield);
            if let Some(owner) = owner {
                super::metafields::emit_change(state, &owner, &metafield, "delete");
            }
        }
    }
    state.delete(&id, "metaobject_definitions/delete");
    Ok(answer(json!({"deletedId": id}), &[]))
}

// ---- entries -----------------------------------------------------------------------------

/// A field of an entry without a value.
fn empty_field(definition: &Value) -> Value {
    json!({
        "key": definition.get("key").cloned().unwrap_or(Value::Null),
        "type": definition.pointer("/type/name").cloned().unwrap_or(Value::Null),
        "value": null,
        "jsonValue": null,
        "definition": definition,
    })
}

/// The display name of an entry: the value of its display name field, else its handle,
/// humanized (as the seed names them).
fn display_name(fields: &[Value], display: Option<&str>, handle: &str) -> String {
    display
        .and_then(|key| {
            fields
                .iter()
                .find(|field| field.get("key").and_then(Value::as_str) == Some(key))
        })
        .and_then(|field| field.get("value").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .map_or_else(|| value::humanize(handle), str::to_string)
}

/// The values an input gives: `fields` (`[{key, value}]`) or `values` (a JSON object, whose
/// values are strings or JSON, written as the string of their JSON), in order, and whether they
/// replace every value (`values`).
fn given_values(
    input: &Map<String, Value>,
    values: Option<&Value>,
    errors: &mut Vec<MetaobjectError>,
    path: &str,
) -> (Vec<(String, Option<String>)>, bool) {
    let fields = input.get("fields").filter(|fields| !fields.is_null());
    let values = values.filter(|values| !values.is_null());
    if fields.is_some() && values.is_some() {
        errors.push(MetaobjectError::new(
            &[path],
            "Fields and values cannot be used together.",
            "INVALID",
        ));
        return (Vec::new(), false);
    }
    if let Some(Value::Object(values)) = values {
        let given = values
            .iter()
            .map(|(key, value)| {
                let text = match value {
                    Value::Null => None,
                    Value::String(text) => Some(text.clone()),
                    other => Some(other.to_string()),
                };
                (key.clone(), text)
            })
            .collect();
        return (given, true);
    }
    let given = fields
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|field| {
            (
                field
                    .get("key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                field
                    .get("value")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        })
        .collect();
    (given, false)
}

/// The fields of an entry once the values given are applied (checked against the definition):
/// every field of the definition, in its order.
fn apply_values(
    state: &ShopState,
    definition: &Record,
    held: &[Value],
    given: &[(String, Option<String>)],
    replace: bool,
    path: &str,
    errors: &mut Vec<MetaobjectError>,
) -> Vec<Value> {
    let definitions: Vec<Value> = definition
        .get("fieldDefinitions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let currency = shop_currency(state);
    let mut seen: Vec<&str> = Vec::new();
    for (index, (key, _)) in given.iter().enumerate() {
        let index = index.to_string();
        if seen.contains(&key.as_str()) {
            errors.push(
                MetaobjectError::new(
                    &[path, "fields", &index],
                    "Duplicate field input.",
                    "DUPLICATE_FIELD_INPUT",
                )
                .key(key),
            );
        }
        seen.push(key);
        if !definitions
            .iter()
            .any(|field| field.get("key").and_then(Value::as_str) == Some(key.as_str()))
        {
            errors.push(
                MetaobjectError::new(
                    &[path, "fields", &index],
                    format!("Field definition \"{key}\" does not exist"),
                    "UNDEFINED_OBJECT_FIELD",
                )
                .key(key),
            );
        }
    }
    definitions
        .iter()
        .map(|field_definition| {
            let key = field_definition
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let kind = field_definition
                .pointer("/type/name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let given_at = given.iter().position(|(given, _)| given == key);
            let held_field = held
                .iter()
                .find(|field| field.get("key").and_then(Value::as_str) == Some(key))
                .cloned();
            let mut field = match given_at {
                Some(at) => match &given[at].1 {
                    Some(raw) if !raw.is_empty() => {
                        let validations: Vec<Value> = field_definition
                            .get("validations")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let cx = Context {
                            state,
                            currency: &currency,
                            validations: &validations,
                        };
                        match types::check(kind, raw, &cx) {
                            Ok(checked) => {
                                let mut field = json!({
                                    "key": key,
                                    "type": kind,
                                    "value": checked.value,
                                    "jsonValue": checked.json,
                                });
                                if types::is_reference(kind) {
                                    if types::is_list(kind) {
                                        field["references"] =
                                            json!(checked.references.unwrap_or_default());
                                    } else {
                                        field["reference"] = json!(checked.reference);
                                    }
                                }
                                field
                            }
                            Err(invalid) => {
                                let index = at.to_string();
                                errors.push(
                                    MetaobjectError::new(
                                        &[path, "fields", &index],
                                        invalid.message,
                                        "INVALID_VALUE",
                                    )
                                    .key(key),
                                );
                                empty_field(field_definition)
                            }
                        }
                    }
                    _ => empty_field(field_definition),
                },
                None if replace => empty_field(field_definition),
                None => held_field.unwrap_or_else(|| empty_field(field_definition)),
            };
            field["definition"] = field_definition.clone();
            let required = field_definition
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if required && field.get("value").is_none_or(Value::is_null) {
                errors.push(
                    MetaobjectError::new(
                        &[path],
                        format!("Field \"{key}\" is required."),
                        "OBJECT_FIELD_REQUIRED",
                    )
                    .key(key),
                );
            }
            field
        })
        .collect()
}

/// A handle as Shopify keeps one: lowercase words joined by `-`.
fn handle_of(text: &str) -> String {
    lsf_core::util::handleize(text)
}

/// A free handle of a type made from `base`: itself, else with `-1`, `-2`, … appended.
fn free_handle(state: &ShopState, kind: &str, base: &str, except: Option<&str>) -> String {
    crate::admin::state::free_handle(base, |handle| {
        entry_of(state, kind, handle).is_some_and(|id| Some(id.as_str()) != except)
    })
}

/// The capabilities of an entry: the input's publishable status (a `DRAFT` by default when the
/// definition is publishable) and online store template suffix, checked against the definition.
fn entry_capabilities(
    definition: &Record,
    input: Option<&Value>,
    before: Option<&Value>,
    path: &str,
    errors: &mut Vec<MetaobjectError>,
) -> Value {
    let enabled = |name: &str| {
        definition
            .get("capabilities")
            .and_then(|capabilities| capabilities.get(name))
            .and_then(|capability| capability.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let mut capabilities = before
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let status = input.and_then(|input| input.pointer("/publishable/status"));
    if let Some(status) = status.filter(|status| !status.is_null()) {
        if enabled("publishable") {
            capabilities.insert("publishable".into(), json!({"status": status}));
        } else {
            errors.push(MetaobjectError::new(
                &[path, "capabilities", "publishable"],
                "Capability is not enabled on the definition.",
                "CAPABILITY_NOT_ENABLED",
            ));
        }
    } else if enabled("publishable") && !capabilities.contains_key("publishable") {
        capabilities.insert("publishable".into(), json!({"status": "DRAFT"}));
    }
    if let Some(online) = input
        .and_then(|input| input.get("onlineStore"))
        .filter(|online| !online.is_null())
    {
        if enabled("onlineStore") {
            capabilities.insert(
                "onlineStore".into(),
                json!({"templateSuffix": online.get("templateSuffix").cloned().unwrap_or(Value::Null)}),
            );
        } else {
            errors.push(MetaobjectError::new(
                &[path, "capabilities", "onlineStore"],
                "Capability is not enabled on the definition.",
                "CAPABILITY_NOT_ENABLED",
            ));
        }
    }
    Value::Object(capabilities)
}

/// What a create or an update has checked, ready to write.
struct Planned {
    handle: String,
    fields: Vec<Value>,
    capabilities: Value,
    display_name: String,
}

/// Checks an entry's input against its definition: `existing` is the entry an update changes.
#[allow(clippy::too_many_arguments)]
fn plan(
    state: &ShopState,
    definition: &Record,
    existing: Option<(&str, &Record)>,
    input: &Map<String, Value>,
    values: Option<&Value>,
    handle: Option<&str>,
    path: &str,
    errors: &mut Vec<MetaobjectError>,
) -> Planned {
    let kind = text(definition, "type");
    let (given, replace) = given_values(input, values, errors, path);
    let held: Vec<Value> = existing
        .and_then(|(_, record)| record.get("fields"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let fields = apply_values(state, definition, &held, &given, replace, path, errors);
    let display = definition.get("displayNameKey").and_then(Value::as_str);
    let handle = match handle.filter(|handle| !handle.trim().is_empty()) {
        Some(given) => {
            let handle = handle_of(given);
            if entry_of(state, kind, &handle)
                .is_some_and(|id| Some(id.as_str()) != existing.map(|(id, _)| id))
            {
                errors.push(MetaobjectError::new(
                    &[path, "handle"],
                    "Handle has already been taken",
                    "TAKEN",
                ));
            }
            handle
        }
        None => match existing {
            Some((_, record)) => text(record, "handle").to_string(),
            None => {
                let named = display_name(&fields, display, "");
                let base = match handle_of(&named) {
                    base if base.is_empty() => handle_of(kind.rsplit("--").next().unwrap_or(kind)),
                    base => base,
                };
                free_handle(state, kind, &base, None)
            }
        },
    };
    let capabilities = entry_capabilities(
        definition,
        input.get("capabilities"),
        existing.and_then(|(_, record)| record.get("capabilities")),
        path,
        errors,
    );
    Planned {
        display_name: display_name(&fields, display, &handle),
        handle,
        fields,
        capabilities,
    }
}

/// Inserts a new entry of a definition: its id `metaobject_gid(type, handle)`, else a new one.
fn insert_entry(state: &mut ShopState, definition: &str, kind: &str, planned: Planned) -> String {
    let now = state.timestamp();
    let stable = metaobject_gid(&data_type(state.app_id(), kind), &planned.handle);
    let id = state.created_gid("Metaobject", stable);
    let app = state.app_gid().map(str::to_string);
    state.insert(value::object(json!({
        "__typename": "Metaobject",
        "id": id,
        "type": kind,
        "handle": planned.handle,
        "displayName": planned.display_name,
        "definition": definition,
        "fields": planned.fields,
        "capabilities": planned.capabilities,
        "createdByApp": app,
        "createdAt": now,
        "updatedAt": now,
    })));
    if let Some(record) = state.get_mut(definition) {
        if let Some(Value::Array(entries)) = record.get_mut("metaobjects") {
            entries.push(json!(id));
        } else {
            record.insert("metaobjects".into(), json!([id]));
        }
        let count = record
            .get("metaobjects")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        record.insert("metaobjectsCount".into(), json!(count));
    }
    state.emit("metaobjects/create", &id);
    id
}

fn write_entry(state: &mut ShopState, id: &str, planned: Planned) {
    if let Some(record) = state.get_mut(id) {
        record.insert("handle".into(), json!(planned.handle));
        record.insert("displayName".into(), json!(planned.display_name));
        record.insert("fields".into(), json!(planned.fields));
        record.insert("capabilities".into(), planned.capabilities);
    }
    state.touch(id);
    state.emit("metaobjects/update", id);
}

/// Removes an entry and its place in its definition: `metaobjects/delete`.
fn delete_entry(state: &mut ShopState, id: &str) {
    let definition = state
        .get(id)
        .map(|record| text(record, "definition").to_string());
    state.delete(id, "metaobjects/delete");
    if let Some(record) = definition.and_then(|definition| state.get_mut(&definition)) {
        if let Some(Value::Array(entries)) = record.get_mut("metaobjects") {
            entries.retain(|entry| entry.as_str() != Some(id));
        }
        let count = record
            .get("metaobjects")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        record.insert("metaobjectsCount".into(), json!(count));
    }
}

fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("metaobject");
    let state = mutation.state();
    let given_type = input
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = stored_type(state, given_type);
    let Some(definition) = definition_of(state, &kind) else {
        let error = MetaobjectError::new(
            &["metaobject", "type"],
            format!("No metaobject definition exists for type \"{given_type}\""),
            "UNDEFINED_OBJECT_TYPE",
        );
        return Ok(answer(json!({"metaobject": null}), &[error]));
    };
    let definition_record = state.get(&definition).cloned().unwrap_or_default();
    let mut errors = Vec::new();
    let planned = plan(
        state,
        &definition_record,
        None,
        &input,
        input.get("values"),
        input.get("handle").and_then(Value::as_str),
        "metaobject",
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(answer(json!({"metaobject": null}), &errors));
    }
    let id = insert_entry(mutation.state_mut(), &definition, &kind, planned);
    Ok(answer(json!({"metaobject": id}), &[]))
}

fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = ctx.arg_object_or_default("metaobject");
    let state = mutation.state();
    let Some(record) = state.get_of(&id, "Metaobject").cloned() else {
        let error = MetaobjectError::new(&["id"], "Record not found", "RECORD_NOT_FOUND");
        return Ok(answer(json!({"metaobject": null}), &[error]));
    };
    let definition = state
        .get(text(&record, "definition"))
        .cloned()
        .unwrap_or_default();
    let mut errors = Vec::new();
    let planned = plan(
        state,
        &definition,
        Some((&id, &record)),
        &input,
        input.get("values"),
        input.get("handle").and_then(Value::as_str),
        "metaobject",
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(answer(json!({"metaobject": null}), &errors));
    }
    if input.get("redirectNewHandle").and_then(Value::as_bool) == Some(true) {
        mutation
            .note("metaobjectUpdate: lsf does not create the redirect of `redirectNewHandle` yet");
    }
    write_entry(mutation.state_mut(), &id, planned);
    Ok(answer(json!({"metaobject": id}), &[]))
}

/// `metaobjectUpsert`: the entry of the type and handle is updated (`metaobject.fields` patch it,
/// `values` replace its values), or created.
fn upsert(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("metaobject");
    let values = ctx.arg("values");
    let get = |name: &str| {
        ctx.arg("handle")
            .and_then(|handle| handle.get(name))
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let state = mutation.state();
    let given_type = get("type");
    let kind = stored_type(state, given_type);
    let Some(definition) = definition_of(state, &kind) else {
        let error = MetaobjectError::new(
            &["handle", "type"],
            format!("No metaobject definition exists for type \"{given_type}\""),
            "UNDEFINED_OBJECT_TYPE",
        );
        return Ok(answer(json!({"metaobject": null}), &[error]));
    };
    let definition_record = state.get(&definition).cloned().unwrap_or_default();
    let handle = handle_of(get("handle"));
    let existing = entry_of(state, &kind, &handle);
    let existing_record = existing.as_deref().and_then(|id| state.get(id)).cloned();
    let mut errors = Vec::new();
    if ctx.arg("metaobject").is_some() && values.is_some() {
        errors.push(MetaobjectError::new(
            &["values"],
            "Fields and values cannot be used together.",
            "INVALID",
        ));
        return Ok(answer(json!({"metaobject": null}), &errors));
    }
    let new_handle = input
        .get("handle")
        .and_then(Value::as_str)
        .map_or(handle.as_str(), |given| given);
    let planned = plan(
        state,
        &definition_record,
        existing.as_deref().zip(existing_record.as_ref()),
        &input,
        values,
        Some(new_handle),
        "metaobject",
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(answer(json!({"metaobject": null}), &errors));
    }
    let state = mutation.state_mut();
    let id = match existing {
        Some(id) => {
            write_entry(state, &id, planned);
            id
        }
        None => insert_entry(state, &definition, &kind, planned),
    };
    Ok(answer(json!({"metaobject": id}), &[]))
}

fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if mutation.state().type_of(&id) != Some("Metaobject") {
        let error = MetaobjectError::new(&["id"], "Record not found", "RECORD_NOT_FOUND");
        return Ok(answer(json!({"deletedId": null}), &[error]));
    }
    delete_entry(mutation.state_mut(), &id);
    Ok(answer(json!({"deletedId": id}), &[]))
}

/// `metaobjectBulkDelete(where: {ids} | {type})`: done at once; the `Job` is `done`.
fn bulk_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let condition = ctx.arg("where").cloned().unwrap_or_default();
    let state = mutation.state();
    let ids: Vec<String> = match (
        condition.get("ids").and_then(Value::as_array),
        condition.get("type").and_then(Value::as_str),
    ) {
        (Some(ids), None) => ids
            .iter()
            .filter_map(Value::as_str)
            .filter(|id| state.type_of(id) == Some("Metaobject"))
            .map(str::to_string)
            .collect(),
        (None, Some(kind)) => {
            let kind = stored_type(state, kind);
            if definition_of(state, &kind).is_none() {
                let error = MetaobjectError::new(
                    &["where", "type"],
                    format!("No metaobject definition exists for type \"{kind}\""),
                    "UNDEFINED_OBJECT_TYPE",
                );
                return Ok(answer(json!({"job": null}), &[error]));
            }
            state
                .keys("Metaobject")
                .filter(|id| {
                    state
                        .get(id)
                        .is_some_and(|entry| text(entry, "type") == kind)
                })
                .map(str::to_string)
                .collect()
        }
        _ => {
            let error = MetaobjectError::new(
                &["where"],
                "Exactly one of ids or type must be given.",
                "INVALID",
            );
            return Ok(answer(json!({"job": null}), &[error]));
        }
    };
    let state = mutation.state_mut();
    for id in &ids {
        delete_entry(state, id);
    }
    let job = crate::admin::resolve::done_job(state, "metaobject_bulk_delete");
    Ok(answer(json!({ "job": job }), &[]))
}
