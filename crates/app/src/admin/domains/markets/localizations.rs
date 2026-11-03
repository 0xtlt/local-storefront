//! Market localizations: `marketLocalizationsRegister`, `marketLocalizationsRemove`, the
//! `marketLocalizableResource`, `marketLocalizableResources` and
//! `marketLocalizableResourcesByIds` queries, and a resource's `marketLocalizableContent` and
//! `marketLocalizations(marketId:)`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - The market localizable resources are metafields (their `value`) and metaobjects (their
//!   fields: "Market Localizable fields are determined by the Metaobject type"; lsf takes every
//!   field of the entry). Each key has a value and a digest: the SHA-256 of the value in hex,
//!   as the digests of translatable content.
//! - `marketLocalizationsRegister` takes a value per key and market: the market must exist
//!   (`MARKET_DOES_NOT_EXIST`), the key be one of the resource's (`INVALID_KEY_FOR_MODEL`), the
//!   digest be that of the content now (`INVALID_MARKET_LOCALIZABLE_CONTENT`), the value not be
//!   blank. A localization of the same key and market is replaced.
//! - `marketLocalizationsRemove` removes those of the keys and markets given, and answers them.
//! - A localization is `outdated` when the resource changed after it was registered.
//!
//! They are kept like translations, in a record per resource: a `MarketLocalizableResource`
//! (`resourceId`, `lsfLocalizations`) under `market_localizations/<resource GID>`. They are not
//! store data: the storefront does not show them. Shopify has no webhook topic for them: their
//! change is the event `market_localizations/update` on the resource, which no subscription can
//! name. The messages are lsf's, in the words of those of `translationsRegister`.

use serde_json::{Map, Value, json};

use super::strings;
use crate::admin::domains::content::translations::{changed_at, digest, is_outdated, resource_of};
use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::admin::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .query("marketLocalizableResource", resource)
        .query("marketLocalizableResources", resources)
        .query("marketLocalizableResourcesByIds", resources_by_ids)
        .field(
            "MarketLocalizableResource",
            "marketLocalizableContent",
            localizable_content,
        )
        .field(
            "MarketLocalizableResource",
            "marketLocalizations",
            localizations,
        )
        .mutation("marketLocalizationsRegister", register_localizations)
        .mutation("marketLocalizationsRemove", remove_localizations);
}

/// The market localizable content of a record: `(key, value)`. `None` for a record that is not
/// market localizable.
fn content(state: &ShopState, key: &str) -> Option<Vec<(String, String)>> {
    let record = state.get(key)?;
    match state.type_of(key)? {
        "Metafield" => Some(vec![(
            "value".to_string(),
            record
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )]),
        "Metaobject" => Some(
            record
                .get("fields")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|field| {
                    Some((
                        field.get("key")?.as_str()?.to_string(),
                        field
                            .get("value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ))
                })
                .collect(),
        ),
        _ => None,
    }
}

/// The key of the record that keeps a resource's localizations.
fn key_of(resource: &str) -> String {
    format!("market_localizations/{resource}")
}

/// The localizations of a resource, as kept.
fn kept(state: &ShopState, resource: &str) -> Vec<Value> {
    state
        .get(&key_of(resource))
        .and_then(|record| record.get("lsfLocalizations"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Keeps the localizations of a resource (none: its record goes).
fn store(state: &mut ShopState, resource: &str, localizations: Vec<Value>) {
    let key = key_of(resource);
    if localizations.is_empty() {
        state.remove(&key);
        return;
    }
    state.insert_as(
        key,
        value::object(json!({
            "__typename": "MarketLocalizableResource",
            "resourceId": resource,
            "lsfLocalizations": localizations,
        })),
    );
}

/// A `MarketLocalizableResource` object of a resource.
fn resource_object(resource: &str) -> Obj {
    let mut value = Record::new();
    value.insert("resourceId".into(), json!(resource));
    Obj::built("MarketLocalizableResource", value)
}

fn resource(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let resource = ctx.arg_str("resourceId").unwrap_or_default();
    Ok(Output::object_or_null(
        content(r.state(), resource).map(|_| resource_object(resource)),
    ))
}

fn resources(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let type_name = match ctx.arg_str("resourceType") {
        Some("METAOBJECT") => "Metaobject",
        _ => "Metafield",
    };
    let items = r
        .state()
        .keys(type_name)
        .map(|key| json!({"resourceId": key}))
        .collect();
    r.connection(ctx, items)
}

fn resources_by_ids(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = strings(ctx.arg("resourceIds"))
        .into_iter()
        .filter(|id| content(r.state(), id).is_some())
        .map(|id| json!({"resourceId": id}))
        .collect();
    r.connection(ctx, items)
}

fn localizable_content(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let resource = resource_of(r, object);
    let entries = content(r.state(), &resource).unwrap_or_default();
    Ok(Output::objects(entries.into_iter().map(|(key, value)| {
        Obj::built(
            "MarketLocalizableContent",
            value::object(json!({"key": key, "digest": digest(&value), "value": value})),
        )
    })))
}

/// The localizations of a resource for a market, with `outdated`.
fn for_market(state: &ShopState, resource: &str, market: &str) -> Vec<Map<String, Value>> {
    let changed = changed_at(state, resource);
    kept(state, resource)
        .into_iter()
        .filter_map(|localization| localization.as_object().cloned())
        .filter(|localization| localization.get("market").and_then(Value::as_str) == Some(market))
        .map(|mut localization| {
            let outdated = is_outdated(changed, localization.get("updatedAt"));
            localization.insert("outdated".into(), json!(outdated));
            localization
        })
        .collect()
}

fn localizations(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let resource = resource_of(r, object);
    let market = ctx.arg_str("marketId").unwrap_or_default();
    Ok(Output::objects(
        for_market(r.state(), &resource, market)
            .into_iter()
            .map(|localization| Obj::built("MarketLocalization", localization)),
    ))
}

fn not_found(resource: &str) -> UserError {
    UserError::new(
        &["resourceId"],
        format!("Resource {resource} does not exist"),
    )
    .code("RESOURCE_NOT_FOUND")
}

/// `marketLocalizationsRegister`.
fn register_localizations(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let resource = ctx.arg_str("resourceId").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(entries) = content(state, &resource) else {
        let error = if state.contains(&resource) {
            UserError::new(
                &["resourceId"],
                format!("Resource {resource} is not market customizable"),
            )
            .code("RESOURCE_NOT_MARKET_CUSTOMIZABLE")
        } else {
            not_found(&resource)
        };
        return Ok(payload(json!({"marketLocalizations": null}), &[error]));
    };
    let inputs = super::objects(ctx.arg("marketLocalizations"));
    let mut errors = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let index = index.to_string();
        let get = |name: &str| input.get(name).and_then(Value::as_str).unwrap_or_default();
        let error = |field: &str, message: String, code: &str| UserError {
            field: Some(vec![
                "marketLocalizations".to_string(),
                index.clone(),
                field.to_string(),
            ]),
            message,
            code: Some(code.to_string()),
        };
        if !state.is_of(get("marketId"), "Market") {
            errors.push(error(
                "marketId",
                format!("Market {} does not exist", get("marketId")),
                "MARKET_DOES_NOT_EXIST",
            ));
        }
        match entries.iter().find(|(key, _)| key == get("key")) {
            None => errors.push(error(
                "key",
                format!("Key {} is not a valid market localizable field", get("key")),
                "INVALID_KEY_FOR_MODEL",
            )),
            Some((_, value)) if digest(value) != get("marketLocalizableContentDigest") => {
                errors.push(error(
                    "marketLocalizableContentDigest",
                    "Market localizable content hash is invalid".to_string(),
                    "INVALID_MARKET_LOCALIZABLE_CONTENT",
                ));
            }
            Some(_) => {}
        }
        if get("value").trim().is_empty() {
            errors.push(error("value", "Value can't be blank".to_string(), "BLANK"));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"marketLocalizations": null}), &errors));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let mut all = kept(state, &resource);
    let mut registered = Vec::new();
    for input in &inputs {
        let get = |name: &str| input.get(name).and_then(Value::as_str);
        let localization = json!({
            "key": get("key"),
            "value": get("value"),
            "market": get("marketId"),
            "outdated": false,
            "updatedAt": now,
        });
        all.retain(|known| {
            !(known.get("key") == localization.get("key")
                && known.get("market") == localization.get("market"))
        });
        all.push(localization.clone());
        registered.push(localization);
    }
    store(state, &resource, all);
    state.emit("market_localizations/update", &resource);
    Ok(payload(json!({"marketLocalizations": registered}), &[]))
}

/// `marketLocalizationsRemove`.
fn remove_localizations(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let resource = ctx.arg_str("resourceId").unwrap_or_default().to_string();
    let state = mutation.state();
    if content(state, &resource).is_none() {
        return Ok(payload(
            json!({"marketLocalizations": null}),
            &[not_found(&resource)],
        ));
    }
    let keys = strings(ctx.arg("marketLocalizationKeys"));
    let markets = strings(ctx.arg("marketIds"));
    let errors: Vec<UserError> = markets
        .iter()
        .enumerate()
        .filter(|(_, market)| !state.is_of(market, "Market"))
        .map(|(index, market)| {
            UserError::new(
                &["marketIds", &index.to_string()],
                format!("Market {market} does not exist"),
            )
            .code("MARKET_DOES_NOT_EXIST")
        })
        .collect();
    if !errors.is_empty() {
        return Ok(payload(json!({"marketLocalizations": null}), &errors));
    }
    let matches = |localization: &Value| {
        let field = |name: &str| localization.get(name).and_then(Value::as_str);
        field("key").is_some_and(|key| keys.iter().any(|wanted| wanted == key))
            && field("market").is_some_and(|market| markets.iter().any(|wanted| wanted == market))
    };
    let all = kept(state, &resource);
    let removed: Vec<Value> = all.iter().filter(|known| matches(known)).cloned().collect();
    if removed.is_empty() {
        return Ok(payload(json!({"marketLocalizations": []}), &[]));
    }
    let remaining: Vec<Value> = all.into_iter().filter(|known| !matches(known)).collect();
    let state = mutation.state_mut();
    store(state, &resource, remaining);
    state.emit("market_localizations/update", &resource);
    Ok(payload(json!({"marketLocalizations": removed}), &[]))
}
