//! Mobile platform applications: `mobilePlatformApplicationCreate`, `Update`, `Delete`;
//! `mobilePlatformApplication(id:)`, `mobilePlatformApplications`.
//!
//! An `AndroidApplication` or an `AppleApplication` (the reference's examples: an id of
//! `MobilePlatformApplication`, the type by platform), which is what tells Android and iOS that
//! the app may open the shop's links: Shopify serves the association files of the online
//! store's domain from them. lsf serves them too, from the default shop
//! ([`association_file`], `/.well-known/assetlinks.json` and
//! `/.well-known/apple-app-site-association` on the storefront), in the formats Google
//! ("Digital Asset Links") and Apple ("Supporting associated domains") define: no phone ever
//! fetches them from lsf, but a test reads what a phone would.
//!
//! Validation: the input names one platform (the schema's `@oneOf`); an Apple app with app
//! clips needs the clip's id, values are 255 characters at most (`TOO_LONG`). Shopify documents
//! no message for either: the wording is lsf's.

use serde_json::{Map, Value, json};

use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("mobilePlatformApplicationCreate", create)
        .mutation("mobilePlatformApplicationUpdate", update)
        .mutation("mobilePlatformApplicationDelete", delete);
}

const TYPES: [&str; 2] = ["AndroidApplication", "AppleApplication"];

/// The longest value Shopify keeps in a text column.
const MAX_LENGTH: usize = 255;

fn record_of(state: &ShopState, id: &str) -> Option<Record> {
    state
        .type_of(id)
        .filter(|kind| TYPES.contains(kind))
        .and_then(|_| state.get(id).cloned())
}

/// The platform of an input (`android` or `apple`) and its fields.
fn platform(ctx: &FieldContext<'_>) -> Option<(&'static str, Map<String, Value>)> {
    let input = ctx.arg_object("input")?;
    for (name, kind) in [
        ("android", "AndroidApplication"),
        ("apple", "AppleApplication"),
    ] {
        if let Some(Value::Object(fields)) = input.get(name) {
            return Some((kind, fields.clone()));
        }
    }
    None
}

/// The record of an input over `before` (an update), checked.
fn merged(
    kind: &str,
    fields: &Map<String, Value>,
    before: Option<&Record>,
    errors: &mut Vec<UserError>,
) -> Record {
    let input_name = if kind == "AndroidApplication" {
        "android"
    } else {
        "apple"
    };
    let mut record = before.cloned().unwrap_or_else(|| {
        value::object(if kind == "AndroidApplication" {
            json!({"__typename": kind, "applicationId": null, "appLinksEnabled": false, "sha256CertFingerprints": []})
        } else {
            json!({"__typename": kind, "appId": null, "appClipApplicationId": null, "appClipsEnabled": false, "sharedWebCredentialsEnabled": false, "universalLinksEnabled": false})
        })
    });
    for (key, given) in fields {
        if given.is_null()
            && key != "appId"
            && key != "applicationId"
            && key != "appClipApplicationId"
        {
            continue;
        }
        let too_long = match given {
            Value::String(text) => text.chars().count() > MAX_LENGTH,
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .any(|text| text.chars().count() > MAX_LENGTH),
            _ => false,
        };
        if too_long {
            errors.push(
                UserError::new(
                    &["input", input_name, key],
                    format!("{key} is too long (maximum is {MAX_LENGTH} characters)"),
                )
                .code("TOO_LONG"),
            );
        }
        record.insert(key.clone(), given.clone());
    }
    if kind == "AppleApplication"
        && record.get("appClipsEnabled").and_then(Value::as_bool) == Some(true)
        && record
            .get("appClipApplicationId")
            .and_then(Value::as_str)
            .is_none_or(|id| id.trim().is_empty())
    {
        errors.push(
            UserError::new(
                &["input", "apple", "appClipApplicationId"],
                "App clips need the app clip application ID",
            )
            .code("INVALID"),
        );
    }
    record
}

fn create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let Some((kind, fields)) = platform(ctx) else {
        let error =
            UserError::new(&["input"], "Give the android or the apple application").code("INVALID");
        return Ok(payload(
            json!({"mobilePlatformApplication": null}),
            &[error],
        ));
    };
    let mut errors = Vec::new();
    let mut record = merged(kind, &fields, None, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"mobilePlatformApplication": null}), &errors));
    }
    let state = m.state_mut();
    let id = state.new_gid("MobilePlatformApplication");
    record.insert("id".into(), json!(id));
    state.insert(record);
    state.emit("mobile_platform_applications/create", &id);
    Ok(payload(json!({"mobilePlatformApplication": id}), &[]))
}

fn update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let Some(before) = record_of(m.state(), &id) else {
        let error =
            UserError::new(&["id"], "Mobile platform application not found").code("NOT_FOUND");
        return Ok(payload(
            json!({"mobilePlatformApplication": null}),
            &[error],
        ));
    };
    let mut errors = Vec::new();
    let record = match platform(ctx) {
        Some((kind, fields)) if before.get("__typename").and_then(Value::as_str) == Some(kind) => {
            merged(kind, &fields, Some(&before), &mut errors)
        }
        _ => {
            errors.push(
                UserError::new(&["input"], "The input is not of the application's platform")
                    .code("INVALID"),
            );
            before
        }
    };
    if !errors.is_empty() {
        return Ok(payload(json!({"mobilePlatformApplication": null}), &errors));
    }
    let state = m.state_mut();
    state.insert(record);
    state.emit("mobile_platform_applications/update", &id);
    Ok(payload(json!({"mobilePlatformApplication": id}), &[]))
}

fn delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if record_of(m.state(), &id).is_none() {
        let error =
            UserError::new(&["id"], "Mobile platform application not found").code("NOT_FOUND");
        return Ok(payload(
            json!({"deletedMobilePlatformApplicationId": null}),
            &[error],
        ));
    }
    m.state_mut()
        .delete(&id, "mobile_platform_applications/delete");
    Ok(payload(
        json!({"deletedMobilePlatformApplicationId": id}),
        &[],
    ))
}

/// The association file the storefront serves at `path` from the shop's mobile applications:
/// `/.well-known/assetlinks.json` (Android App Links) or
/// `/.well-known/apple-app-site-association` (Universal Links, shared web credentials, app
/// clips). `None` for another path, or when no application turns it on.
pub fn association_file(state: &ShopState, path: &str) -> Option<Value> {
    match path {
        "/.well-known/assetlinks.json" => {
            let statements: Vec<Value> = state
                .all("AndroidApplication")
                .filter(|app| app.get("appLinksEnabled").and_then(Value::as_bool) == Some(true))
                .filter_map(|app| {
                    Some(json!({
                        "relation": ["delegate_permission/common.handle_all_urls"],
                        "target": {
                            "namespace": "android_app",
                            "package_name": app.get("applicationId")?.as_str()?,
                            "sha256_cert_fingerprints": app.get("sha256CertFingerprints").cloned().unwrap_or_else(|| json!([])),
                        },
                    }))
                })
                .collect();
            (!statements.is_empty()).then(|| Value::Array(statements))
        }
        "/.well-known/apple-app-site-association" | "/apple-app-site-association" => {
            let apps: Vec<&Record> = state.all("AppleApplication").collect();
            let ids = |flag: &str, field: &str| -> Vec<Value> {
                apps.iter()
                    .filter(|app| app.get(flag).and_then(Value::as_bool) == Some(true))
                    .filter_map(|app| app.get(field)?.as_str().map(|id| json!(id)))
                    .collect()
            };
            let links = ids("universalLinksEnabled", "appId");
            let credentials = ids("sharedWebCredentialsEnabled", "appId");
            let clips = ids("appClipsEnabled", "appClipApplicationId");
            if links.is_empty() && credentials.is_empty() && clips.is_empty() {
                return None;
            }
            let mut file = Map::new();
            if !links.is_empty() {
                file.insert(
                    "applinks".into(),
                    json!({
                        "apps": [],
                        "details": links
                            .iter()
                            .map(|id| json!({"appID": id, "paths": ["*"]}))
                            .collect::<Vec<_>>(),
                    }),
                );
            }
            if !credentials.is_empty() {
                file.insert("webcredentials".into(), json!({"apps": credentials}));
            }
            if !clips.is_empty() {
                file.insert("appclips".into(), json!({"apps": clips}));
            }
            Some(Value::Object(file))
        }
        _ => None,
    }
}
