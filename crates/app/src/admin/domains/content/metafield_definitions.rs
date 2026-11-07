//! Metafield definitions: `metafieldDefinitionCreate`, `metafieldDefinitionUpdate`,
//! `metafieldDefinitionDelete`, `metafieldDefinitionPin`, `metafieldDefinitionUnpin`, and the
//! `metafieldDefinitions`, `metafieldDefinition` and `metafieldDefinitionTypes` queries.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `namespace` omitted is the app's reserved namespace; `$app:x` is `app--<app id>--x`.
//! - A definition is unique per owner type, namespace and key. Creating one where metafields
//!   without a definition exist adopts them (they are then checked against it:
//!   `validationStatus`); where they have another type, it is `UNSTRUCTURED_ALREADY_EXISTS`.
//! - `validations` must be among the type's (`metafieldDefinitionTypes`), and `rating` needs
//!   `scale_min` and `scale_max`.
//! - `pin: true` pins the definition at the end (positions from 1); 20 pinned definitions per
//!   owner type at most (Shopify's admin limit).
//! - `metafieldDefinitionDelete` keeps the metafields unless `deleteAllAssociatedMetafields`,
//!   which "must be `true` when deleting definitions under the `$app` namespace", and for the
//!   reference and `id` types (the codes `REFERENCE_TYPE_DELETION_ERROR`, `ID_TYPE_DELETION_ERROR`).
//!
//! Messages: where Shopify does not publish one, the description of the error code in the
//! schema ("The metafield definition was not found."), else Rails' wording ("Name can't be
//! blank"); "Key is in use for Product metafields on the 'custom' namespace." is quoted from
//! answers of Shopify's community.
//!
//! Events: `metafield_definitions/create`, `metafield_definitions/update` (an update, a pin, an
//! unpin), `metafield_definitions/delete`. Deleting the metafields with their definition changes
//! their owners, as `metafieldsDelete` does.
//!
//! Not simulated: `constraints` (definitions of a product category), `access` beyond keeping
//! what is given, the effects of `capabilities` (kept as given), the asynchronous
//! `validationJob` (the validation is done at once).

use serde_json::{Map, Value, json};

use super::metafields::{
    self, check_namespace_and_key, find_definition, other_apps, owner_types_of, stored_namespace,
    text,
};
use super::types;
use crate::admin::domains::catalog::gids;
use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::sort::SortValue;
use crate::admin::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::admin::state::seed::Seed;
use crate::admin::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .query("metafieldDefinitions", metafield_definitions)
        .query("metafieldDefinition", metafield_definition)
        .query("metafieldDefinitionTypes", metafield_definition_types)
        .sort_key("MetafieldDefinition", "PINNED_POSITION", |r, item| {
            // Pinned first, by position; the others after them.
            let position = r
                .item_map(item)
                .and_then(|definition| definition.get("pinnedPosition"))
                .and_then(Value::as_i64);
            SortValue::of(Some(&json!(position.unwrap_or(i64::MAX))))
        })
        .mutation("metafieldDefinitionCreate", create)
        .mutation("metafieldDefinitionUpdate", update)
        .mutation("metafieldDefinitionDelete", delete)
        .mutation("metafieldDefinitionPin", pin)
        .mutation("metafieldDefinitionUnpin", unpin);
}

/// The most definitions pinned per owner type.
const PINNED_LIMIT: usize = 20;

/// The definitions `shopify.app.toml` declares (`[order.metafields.<namespace>.<key>]`): Shopify
/// creates them in every shop of the app, so the seed does. `app` is the app's reserved
/// namespace; `type`, `name`, `description` and `capabilities.cart_to_order_copyable` are read,
/// not `validations` nor `access` (the defaults of an app-owned definition). Unlike Shopify,
/// which keeps a declared definition read-only through the Admin API, the mutations may change
/// it.
pub fn seed_declared_definitions(seed: &mut Seed) {
    let Some(app) = seed.app else {
        return;
    };
    for declared in &app.config.order_metafields {
        let namespace = super::admin_namespace(seed.app_id, &declared.namespace);
        let state = &mut seed.state;
        if find_definition(state, "ORDER", &namespace, &declared.key).is_some()
            || !types::is_type(&declared.kind)
        {
            continue;
        }
        let id = super::metafield_definition_gid("ORDER", &namespace, &declared.key);
        let app_owned = namespace.starts_with("app--");
        state.insert(value::object(json!({
            "__typename": "MetafieldDefinition",
            "id": id,
            "namespace": namespace,
            "key": declared.key,
            "name": declared.name.clone().unwrap_or_else(|| value::humanize(&declared.key)),
            "description": declared.description,
            "ownerType": "ORDER",
            "type": types::definition_type(&declared.kind),
            "validations": [],
            "validationStatus": "ALL_VALID",
            "pinnedPosition": null,
            "useAsCollectionCondition": false,
            "access": access_of(None, app_owned, None),
            "capabilities": capabilities_of(
                Some(&json!({"cartToOrderCopyable": {"enabled": declared.cart_to_order_copyable}})),
                None,
            ),
            "constraints": null,
            "standardTemplate": null,
            "metafields": [],
            "metafieldsCount": 0,
        })));
    }
}

/// `metafieldDefinitions(ownerType:, namespace:, key:, pinnedStatus:, query:, sortKey:)`.
fn metafield_definitions(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let owner_type = ctx.arg_str("ownerType").unwrap_or_default();
    let namespace = ctx.arg_str("namespace").map(|ns| r.app_namespace(ns));
    let key = ctx.arg_str("key");
    let pinned = ctx.arg_str("pinnedStatus").unwrap_or("ANY");
    if ctx.arg("constraintStatus").is_some() || ctx.arg("constraintSubtype").is_some() {
        r.note("metafieldDefinitions: lsf does not simulate category constraints: every definition is unconstrained");
    }
    let items: Vec<Value> = r
        .records_of("MetafieldDefinition")
        .into_iter()
        .filter(|item| {
            let Some(definition) = r.item_map(item) else {
                return false;
            };
            let is_pinned = definition
                .get("pinnedPosition")
                .is_some_and(|position| !position.is_null());
            text(definition, "ownerType") == owner_type
                && namespace
                    .as_deref()
                    .is_none_or(|namespace| text(definition, "namespace") == namespace)
                && key.is_none_or(|key| text(definition, "key") == key)
                && match pinned {
                    "PINNED" => is_pinned,
                    "UNPINNED" => !is_pinned,
                    _ => true,
                }
        })
        .collect();
    r.connection(ctx, items)
}

/// `metafieldDefinition(identifier:)` (or the deprecated `id:`), `$app` read as the app's
/// namespace.
fn metafield_definition(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(identifier) = ctx.arg_object("identifier") else {
        return r.generic(ctx, object);
    };
    let get = |name: &str| identifier.get(name).and_then(Value::as_str);
    let namespace = r.app_namespace(get("namespace").unwrap_or("$app"));
    let found = find_definition(
        r.state(),
        get("ownerType").unwrap_or_default(),
        &namespace,
        get("key").unwrap_or_default(),
    );
    Ok(Output::object_or_null(found.map(|id| Obj::record(&id))))
}

/// `metafieldDefinitionTypes`: every type of 2026-10, in Shopify's order.
fn metafield_definition_types(
    _r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(Output::List(
        types::TYPES
            .iter()
            .map(|kind| {
                let value = value::object(types::definition_type(kind));
                Output::Object(Obj::built("MetafieldDefinitionType", value))
            })
            .collect(),
    ))
}

/// The `MetafieldDefinitionValidation`s of an input (`{name, value}`), with their `type`, checked
/// against what the type supports. `field` is the path of the input's `validations`. The field
/// definitions of metaobjects take the same.
pub(super) fn checked_validations(
    kind: &str,
    input: Option<&Value>,
    field: &[&str],
    errors: &mut Vec<UserError>,
) -> Vec<Value> {
    let supported = types::supported_validations(kind);
    let mut validations: Vec<Value> = Vec::new();
    for (index, validation) in input
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let name = validation
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let value = validation
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let index = index.to_string();
        let mut path: Vec<&str> = field.to_vec();
        path.push(&index);
        let Some((_, value_type)) = supported.iter().find(|(known, _)| *known == name) else {
            errors.push(
                UserError::new(
                    &path,
                    format!("Validations contains an invalid option: '{name}'."),
                )
                .code("INVALID_OPTION"),
            );
            continue;
        };
        if validations
            .iter()
            .any(|known| known.get("name").and_then(Value::as_str) == Some(name))
        {
            errors.push(UserError::new(&path, "A duplicate option.").code("DUPLICATE_OPTION"));
            continue;
        }
        if !valid_option(value_type, value) {
            errors.push(
                UserError::new(
                    &path,
                    format!("Validations has an invalid value for '{name}': '{value}'."),
                )
                .code("INVALID_OPTION"),
            );
            continue;
        }
        validations.push(json!({"name": name, "type": value_type, "value": value}));
    }
    if types::base(kind) == "rating" {
        for required in ["scale_min", "scale_max"] {
            if !validations
                .iter()
                .any(|known| known.get("name").and_then(Value::as_str) == Some(required))
            {
                errors.push(
                    UserError::new(field, format!("Validations must include '{required}'."))
                        .code("INVALID_OPTION"),
                );
            }
        }
    }
    validations
}

/// Whether the value of a validation is one of its type: a number for `min` of a number, a
/// date for a date, JSON for a measurement or a list, a regular expression for `regex`.
fn valid_option(value_type: &str, value: &str) -> bool {
    match value_type {
        "number_integer" => value.parse::<i64>().is_ok(),
        "number_decimal" => value.parse::<f64>().is_ok(),
        "date" => chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok(),
        "date_time" => {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").is_ok()
                || chrono::DateTime::parse_from_rfc3339(value).is_ok()
        }
        "json" => serde_json::from_str::<Value>(value).is_ok(),
        "list.single_line_text_field" => serde_json::from_str::<Vec<String>>(value).is_ok(),
        "single_line_text_field" => !value.is_empty(),
        _ => serde_json::from_str::<Value>(value).is_ok_and(|value| value.is_object()),
    }
}

/// `access` of a metafield or metaobject definition: the input's, with Shopify's defaults (an
/// app's reserved namespace or type is the merchant's to read and write in the admin, another
/// everyone's; nothing on the storefront or in customer accounts unless given).
pub(super) fn access_of(input: Option<&Value>, app_owned: bool, before: Option<&Value>) -> Value {
    let pick = |name: &str, default: &str| {
        input
            .and_then(|access| access.get(name))
            .and_then(Value::as_str)
            .or_else(|| {
                before
                    .and_then(|access| access.get(name))
                    .and_then(Value::as_str)
            })
            .unwrap_or(default)
            .to_string()
    };
    json!({
        "admin": pick("admin", if app_owned { "MERCHANT_READ_WRITE" } else { "PUBLIC_READ_WRITE" }),
        "storefront": pick("storefront", "NONE"),
        "customerAccount": pick("customerAccount", "NONE"),
    })
}

/// `capabilities` of a definition: what the input enables, kept (lsf does not simulate their
/// effects).
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
    let filterable = enabled("adminFilterable");
    json!({
        "adminFilterable": {
            "eligible": true,
            "enabled": filterable,
            "status": if filterable { "FILTERABLE" } else { "NOT_FILTERABLE" },
        },
        "analyticsQueryable": {"eligible": true, "enabled": enabled("analyticsQueryable")},
        "cartToOrderCopyable": {"eligible": true, "enabled": enabled("cartToOrderCopyable")},
        "smartCollectionCondition": {"eligible": true, "enabled": enabled("smartCollectionCondition")},
        "uniqueValues": {"eligible": true, "enabled": enabled("uniqueValues")},
    })
}

/// The pinned definitions of an owner type, by position.
fn pinned(state: &ShopState, owner_type: &str) -> Vec<(String, i64)> {
    let mut pinned: Vec<(String, i64)> = state
        .keys("MetafieldDefinition")
        .filter_map(|id| {
            let definition = state.get(id)?;
            (text(definition, "ownerType") == owner_type)
                .then(|| definition.get("pinnedPosition")?.as_i64())
                .flatten()
                .map(|position| (id.to_string(), position))
        })
        .collect();
    pinned.sort_by_key(|(_, position)| *position);
    pinned
}

/// Pins a definition after the pinned ones.
fn pin_definition(state: &mut ShopState, definition: &str, owner_type: &str) {
    let next = pinned(state, owner_type)
        .last()
        .map_or(1, |(_, position)| position + 1);
    if let Some(record) = state.get_mut(definition) {
        record.insert("pinnedPosition".into(), json!(next));
    }
}

/// Unpins a definition, moving those pinned after it up.
fn unpin_definition(state: &mut ShopState, definition: &str, owner_type: &str) {
    let Some(position) = state
        .get(definition)
        .and_then(|record| record.get("pinnedPosition"))
        .and_then(Value::as_i64)
    else {
        return;
    };
    for (other, other_position) in pinned(state, owner_type) {
        if other_position > position
            && let Some(record) = state.get_mut(&other)
        {
            record.insert("pinnedPosition".into(), json!(other_position - 1));
        }
    }
    if let Some(record) = state.get_mut(definition) {
        record.insert("pinnedPosition".into(), Value::Null);
    }
}

/// The metafields of an owner type, namespace and key, whether or not a definition has them.
fn metafields_of(state: &ShopState, owner_type: &str, namespace: &str, key: &str) -> Vec<String> {
    state
        .keys("Metafield")
        .filter(|id| {
            state.get(id).is_some_and(|metafield| {
                text(metafield, "ownerType") == owner_type
                    && text(metafield, "namespace") == namespace
                    && text(metafield, "key") == key
            })
        })
        .map(str::to_string)
        .collect()
}

/// `access.admin` is the app's to set, on its reserved namespaces only.
fn check_admin_access(input: &Map<String, Value>, app_owned: bool, errors: &mut Vec<UserError>) {
    if input
        .get("access")
        .and_then(|access| access.get("admin"))
        .is_some_and(|admin| !admin.is_null())
        && !app_owned
    {
        errors.push(
            UserError::new(
                &["definition", "access", "admin"],
                "Admin access can only be specified for app-owned metafield definitions.",
            )
            .code("ADMIN_ACCESS_INPUT_NOT_ALLOWED"),
        );
    }
}

/// The checks of a definition's name and description.
fn check_name(input: &Map<String, Value>, required: bool, errors: &mut Vec<UserError>) {
    match input.get("name").and_then(Value::as_str) {
        Some(name) if name.trim().is_empty() => errors
            .push(UserError::new(&["definition", "name"], "Name can't be blank").code("BLANK")),
        None if required => errors
            .push(UserError::new(&["definition", "name"], "Name can't be blank").code("BLANK")),
        Some(name) if name.chars().count() > 255 => errors.push(
            UserError::new(
                &["definition", "name"],
                "Name is too long (maximum is 255 characters)",
            )
            .code("TOO_LONG"),
        ),
        _ => {}
    }
    if input
        .get("description")
        .and_then(Value::as_str)
        .is_some_and(|description| description.chars().count() > 255)
    {
        errors.push(
            UserError::new(
                &["definition", "description"],
                "Description is too long (maximum is 255 characters)",
            )
            .code("TOO_LONG"),
        );
    }
}

fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("definition");
    let get = |name: &str| input.get(name).and_then(Value::as_str);
    let state = mutation.state();
    let mut errors = Vec::new();
    let owner_type = get("ownerType").unwrap_or_default().to_string();
    let key = get("key").unwrap_or_default().to_string();
    let kind = get("type").unwrap_or_default().to_string();
    check_name(&input, true, &mut errors);
    check_namespace_and_key(
        get("namespace"),
        &key,
        &["definition"],
        &mut errors,
        "INVALID_CHARACTER",
    );
    let namespace = stored_namespace(state, get("namespace"));
    let app_owned = namespace.starts_with("app--");
    if other_apps(state, &namespace) {
        errors.push(
            UserError::new(
                &["definition", "namespace"],
                "Access to this namespace and key on Metafields for this resource type is not allowed.",
            )
            .code("INVALID"),
        );
    }
    if namespace == "shopify" || namespace.starts_with("shopify--") {
        errors.push(
            UserError::new(
                &["definition", "namespace"],
                "This namespace and key combination is reserved for standard definitions.",
            )
            .code("RESERVED_NAMESPACE_KEY"),
        );
    }
    if owner_types_of(&owner_type).is_empty() || owner_type == "MEDIA_IMAGE" {
        errors.push(
            UserError::new(
                &["definition", "ownerType"],
                "Owner type can't be used in this mutation.",
            )
            .code("INCLUSION"),
        );
    }
    if !types::is_type(&kind) {
        errors.push(
            UserError::new(&["definition", "type"], "Type isn't included in the list")
                .code("INCLUSION"),
        );
    }
    let validations = checked_validations(
        &kind,
        input.get("validations"),
        &["definition", "validations"],
        &mut errors,
    );
    check_admin_access(&input, app_owned, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"createdDefinition": null}), &errors));
    }
    if find_definition(state, &owner_type, &namespace, &key).is_some() {
        let resource = owner_types_of(&owner_type)
            .first()
            .copied()
            .unwrap_or("Shop");
        let error = UserError::new(
            &["definition", "key"],
            format!("Key is in use for {resource} metafields on the '{namespace}' namespace."),
        )
        .code("TAKEN");
        return Ok(payload(json!({"createdDefinition": null}), &[error]));
    }
    let existing = metafields_of(state, &owner_type, &namespace, &key);
    if existing.iter().any(|id| {
        state
            .get(id)
            .is_some_and(|metafield| text(metafield, "type") != kind)
    }) {
        let error = UserError::new(
            &["definition"],
            "This namespace and key combination is already in use for a set of your metafields.",
        )
        .code("UNSTRUCTURED_ALREADY_EXISTS");
        return Ok(payload(json!({"createdDefinition": null}), &[error]));
    }
    let pin = input.get("pin").and_then(Value::as_bool).unwrap_or(false);
    if pin && pinned(state, &owner_type).len() >= PINNED_LIMIT {
        let error = UserError::new(
            &["definition", "pin"],
            "The pinned limit has been reached for the owner type.",
        )
        .code("PINNED_LIMIT_REACHED");
        return Ok(payload(json!({"createdDefinition": null}), &[error]));
    }
    if input
        .get("constraints")
        .is_some_and(|value| !value.is_null())
    {
        mutation.note(
            "metafieldDefinitionCreate: lsf does not simulate `constraints` yet: it was ignored",
        );
    }

    let state = mutation.state_mut();
    let id = state.new_gid("MetafieldDefinition");
    state.insert(value::object(json!({
        "__typename": "MetafieldDefinition",
        "id": id,
        "namespace": namespace,
        "key": key,
        "name": get("name").unwrap_or_default().trim(),
        "description": get("description"),
        "ownerType": owner_type,
        "type": types::definition_type(&kind),
        "validations": validations,
        "validationStatus": "ALL_VALID",
        "pinnedPosition": null,
        "useAsCollectionCondition": input
            .get("capabilities")
            .and_then(|capabilities| capabilities.get("smartCollectionCondition"))
            .and_then(|capability| capability.get("enabled"))
            .and_then(Value::as_bool)
            .or_else(|| input.get("useAsCollectionCondition").and_then(Value::as_bool))
            .unwrap_or(false),
        "access": access_of(input.get("access"), app_owned, None),
        "capabilities": capabilities_of(input.get("capabilities"), None),
        "constraints": null,
        "standardTemplate": null,
        "metafields": [],
        "metafieldsCount": 0,
    })));
    if pin {
        pin_definition(state, &id, &owner_type);
    }
    for metafield in &existing {
        metafields::link_definition(state, metafield);
    }
    metafields::refresh_definition(state, &id);
    state.emit("metafield_definitions/create", &id);
    Ok(payload(json!({"createdDefinition": id}), &[]))
}

/// The definition a mutation names: `id` (or `definitionId`), else `identifier` (`ownerType`,
/// `namespace`, `key`), else the input's own `ownerType`, `namespace` and `key`.
fn definition_named(
    state: &ShopState,
    id: Option<&str>,
    identifier: Option<&Map<String, Value>>,
) -> Option<String> {
    if let Some(id) = id {
        return (state.type_of(id) == Some("MetafieldDefinition")).then(|| id.to_string());
    }
    let identifier = identifier?;
    let get = |name: &str| identifier.get(name).and_then(Value::as_str);
    let namespace = stored_namespace(state, get("namespace"));
    find_definition(
        state,
        get("ownerType").unwrap_or_default(),
        &namespace,
        get("key").unwrap_or_default(),
    )
}

/// `metafieldDefinitionUpdate`: the definition by `ownerType`, `namespace` and `key`; its name,
/// description, validations (its metafields checked again), pin, access and capabilities.
fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("definition");
    let state = mutation.state();
    let empty = json!({"updatedDefinition": null, "validationJob": null});
    let Some(id) = definition_named(state, None, Some(&input)) else {
        let error = UserError::new(&["definition"], "The metafield definition wasn't found.")
            .code("NOT_FOUND");
        return Ok(payload(empty, &[error]));
    };
    let record = state.get(&id).cloned().unwrap_or_default();
    let kind = record
        .get("type")
        .and_then(|kind| kind.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let owner_type = text(&record, "ownerType").to_string();
    let app_owned = text(&record, "namespace").starts_with("app--");
    let mut errors = Vec::new();
    check_name(&input, false, &mut errors);
    let validations = input.contains_key("validations").then(|| {
        checked_validations(
            &kind,
            input.get("validations"),
            &["definition", "validations"],
            &mut errors,
        )
    });
    check_admin_access(&input, app_owned, &mut errors);
    let pin = input.get("pin").and_then(Value::as_bool);
    let is_pinned = record
        .get("pinnedPosition")
        .is_some_and(|position| !position.is_null());
    if pin == Some(true) && !is_pinned && pinned(state, &owner_type).len() >= PINNED_LIMIT {
        errors.push(
            UserError::new(
                &["definition", "pin"],
                "The pinned limit has been reached for the owner type.",
            )
            .code("PINNED_LIMIT_REACHED"),
        );
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    if input
        .get("constraintsUpdates")
        .is_some_and(|value| !value.is_null())
    {
        mutation.note(
            "metafieldDefinitionUpdate: lsf does not simulate `constraintsUpdates` yet: it was ignored",
        );
    }

    let state = mutation.state_mut();
    if let Some(definition) = state.get_mut(&id) {
        if let Some(name) = input.get("name").and_then(Value::as_str) {
            definition.insert("name".into(), json!(name.trim()));
        }
        if let Some(description) = input.get("description") {
            definition.insert("description".into(), description.clone());
        }
        if let Some(validations) = validations {
            definition.insert("validations".into(), json!(validations));
        }
        if input.contains_key("access") {
            let access = access_of(input.get("access"), app_owned, definition.get("access"));
            definition.insert("access".into(), access);
        }
        if input.contains_key("capabilities") {
            let capabilities =
                capabilities_of(input.get("capabilities"), definition.get("capabilities"));
            let condition = capabilities
                .pointer("/smartCollectionCondition/enabled")
                .cloned()
                .unwrap_or(json!(false));
            definition.insert("capabilities".into(), capabilities);
            definition.insert("useAsCollectionCondition".into(), condition);
        } else if let Some(flag) = input
            .get("useAsCollectionCondition")
            .and_then(Value::as_bool)
        {
            definition.insert("useAsCollectionCondition".into(), json!(flag));
        }
    }
    match pin {
        Some(true) if !is_pinned => pin_definition(state, &id, &owner_type),
        Some(false) if is_pinned => unpin_definition(state, &id, &owner_type),
        _ => {}
    }
    metafields::refresh_definition(state, &id);
    state.emit("metafield_definitions/update", &id);
    Ok(payload(
        json!({"updatedDefinition": id, "validationJob": null}),
        &[],
    ))
}

fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let empty = json!({"deletedDefinitionId": null, "deletedDefinition": null});
    let Some(id) = definition_named(state, ctx.arg_str("id"), ctx.arg_object("identifier")) else {
        let error = UserError::new(&["id"], "Definition not found.").code("NOT_FOUND");
        return Ok(payload(empty, &[error]));
    };
    let record = state.get(&id).cloned().unwrap_or_default();
    let kind = record
        .get("type")
        .and_then(|kind| kind.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let delete_all = ctx
        .arg_bool("deleteAllAssociatedMetafields")
        .unwrap_or(false);
    let metafields = gids(record.get("metafields"));
    let refused = if delete_all {
        None
    } else if text(&record, "namespace").starts_with("app--") {
        Some((
            "Deleting a definition in a reserved namespace requires deletion of its associated metafields.",
            "RESERVED_NAMESPACE_ORPHANED_METAFIELDS",
        ))
    } else if types::is_reference(&kind) && !metafields.is_empty() {
        Some((
            "Deleting a reference type metafield definition requires deletion of its associated metafields.",
            "REFERENCE_TYPE_DELETION_ERROR",
        ))
    } else if kind == "id" && !metafields.is_empty() {
        Some((
            "Deleting an id type metafield definition requires deletion of its associated metafields.",
            "ID_TYPE_DELETION_ERROR",
        ))
    } else {
        None
    };
    if let Some((message, code)) = refused {
        let error = UserError::new(&["deleteAllAssociatedMetafields"], message).code(code);
        return Ok(payload(empty, &[error]));
    }
    let identifier = json!({
        "ownerType": text(&record, "ownerType"),
        "namespace": text(&record, "namespace"),
        "key": text(&record, "key"),
    });

    let state = mutation.state_mut();
    let owner_type = text(&record, "ownerType").to_string();
    unpin_definition(state, &id, &owner_type);
    state.delete(&id, "metafield_definitions/delete");
    for metafield in &metafields {
        if delete_all {
            let owner = state
                .get(metafield)
                .map(|record| text(record, "owner").to_string());
            if let Some(owner) = owner {
                let is_product =
                    matches!(state.type_of(&owner), Some("Product" | "ProductVariant"));
                if !is_product {
                    state.emit("metafields/delete", metafield);
                }
                metafields::remove(state, metafield);
                if is_product {
                    metafields::emit_change(state, &owner, metafield, "delete");
                }
            }
        } else if let Some(record) = state.get_mut(metafield) {
            record.insert("definition".into(), Value::Null);
        }
    }
    Ok(payload(
        json!({"deletedDefinitionId": id, "deletedDefinition": identifier}),
        &[],
    ))
}

fn pin(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let empty = json!({"pinnedDefinition": null});
    let Some(id) = definition_named(
        state,
        ctx.arg_str("definitionId"),
        ctx.arg_object("identifier"),
    ) else {
        let error = UserError::new(&["definitionId"], "The metafield definition was not found.")
            .code("NOT_FOUND");
        return Ok(payload(empty, &[error]));
    };
    let record = state.get(&id).cloned().unwrap_or_default();
    let owner_type = text(&record, "ownerType").to_string();
    if record
        .get("pinnedPosition")
        .is_some_and(|position| !position.is_null())
    {
        let error = UserError::new(
            &["definitionId"],
            "The metafield definition is already pinned.",
        )
        .code("ALREADY_PINNED");
        return Ok(payload(empty, &[error]));
    }
    if pinned(state, &owner_type).len() >= PINNED_LIMIT {
        let error = UserError::new(
            &["definitionId"],
            "The pinned limit has been reached for owner type.",
        )
        .code("PINNED_LIMIT_REACHED");
        return Ok(payload(empty, &[error]));
    }
    let state = mutation.state_mut();
    pin_definition(state, &id, &owner_type);
    state.emit("metafield_definitions/update", &id);
    Ok(payload(json!({"pinnedDefinition": id}), &[]))
}

fn unpin(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let empty = json!({"unpinnedDefinition": null});
    let Some(id) = definition_named(
        state,
        ctx.arg_str("definitionId"),
        ctx.arg_object("identifier"),
    ) else {
        let error = UserError::new(&["definitionId"], "The metafield definition was not found.")
            .code("NOT_FOUND");
        return Ok(payload(empty, &[error]));
    };
    let record = state.get(&id).cloned().unwrap_or_default();
    if record.get("pinnedPosition").is_none_or(Value::is_null) {
        let error = UserError::new(&["definitionId"], "The metafield definition isn't pinned.")
            .code("NOT_PINNED");
        return Ok(payload(empty, &[error]));
    }
    let owner_type = text(&record, "ownerType").to_string();
    let state = mutation.state_mut();
    unpin_definition(state, &id, &owner_type);
    state.emit("metafield_definitions/update", &id);
    Ok(payload(json!({"unpinnedDefinition": id}), &[]))
}
