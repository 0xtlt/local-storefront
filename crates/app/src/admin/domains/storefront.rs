//! What an app adds to the online store: its script tags, its web pixel and server pixel, its
//! Storefront API access tokens.
//!
//! - **Script tags** (`scriptTagCreate`, `scriptTagUpdate`, `scriptTagDelete`; `scriptTag`,
//!   `scriptTags(src:, query:)`): `ScriptTag` records (`src`, `displayScope`, `cache`, dates).
//!   One with the `ONLINE_STORE` or `ALL` scope is loaded by the storefront's
//!   `content_for_header` as Shopify loads it: the server reads [`storefront_scripts`] of its
//!   default shop and gives them to the theme (`lsf_core::theme::Theme::set_script_tags`), once
//!   per change, as store sync builds the store.
//! - **Web pixel** (`webPixelCreate`, `webPixelUpdate`, `webPixelDelete`; `webPixel`): one per
//!   app, as shopify.dev says ("Activate a web pixel extension by creating a web pixel record on
//!   the store"). Its `settings` are checked against the `[settings]` of the app's web pixel
//!   extension (`type = "web_pixel_extension"` in `extensions/*/shopify.extension.toml`), as
//!   "Shopify validates it against the settings definition in `shopify.extension.toml`": every
//!   field defined must be given ("Missing keys: trackingId", `INVALID_SETTINGS`, the example of
//!   the reference), a key the definition lacks is refused, and each value is checked as a
//!   metafield of the field's type and validations is. Without such an extension, Shopify's
//!   `NO_EXTENSION`. The storefront does not run web pixels: lsf has no sandbox for them.
//! - **Server pixel** (`serverPixelCreate`, `serverPixelDelete`, `eventBridgeServerPixelUpdate`,
//!   `pubSubServerPixelUpdate`; `serverPixel`): one per app (`ALREADY_EXISTS`, shopify.dev: "If
//!   you call this mutation when a server pixel already exists, then an error will return"),
//!   `DISCONNECTED_UNCONFIGURED` until an endpoint is set, `CONNECTED` after. lsf sends no
//!   event to the endpoint: the storefront's customer events are not collected.
//! - **Storefront access tokens** (`storefrontAccessTokenCreate`, `storefrontAccessTokenDelete`;
//!   `shop.storefrontAccessTokens`): 32 hex digits, as the reference's examples, with the app's
//!   `unauthenticated_*` scopes ("delegates unauthenticated access scopes to clients using the
//!   Storefront API"), 100 at most per shop ("Each shop can have up to 100 active
//!   `StorefrontAccessToken` objects"). lsf serves no Storefront API, so they authenticate
//!   nothing; they are listed with their scopes.
//!
//! Shopify has no webhook topic for any of these in 2026-10: their changes are emitted under
//! topics no subscription can name (`script_tags/create`, `web_pixels/update`, …), which tell
//! the server the shop changed.
//!
//! Where shopify.dev documents no message, the wording is Rails' (`Src can't be blank`,
//! `Title can't be blank`), the error the REST resources of the same records answered.

use serde_json::{Map, Value, json};

use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::admin::state::value::str_field_or_empty as text;
use crate::admin::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("scriptTagCreate", script_tag_create)
        .mutation("scriptTagUpdate", script_tag_update)
        .mutation("scriptTagDelete", script_tag_delete)
        .query("scriptTags", script_tags)
        .default_text("ScriptTag", |_r, record| {
            record
                .get("src")
                .and_then(Value::as_str)
                .map(str::to_string)
                .into_iter()
                .collect()
        })
        .mutation("webPixelCreate", web_pixel_create)
        .mutation("webPixelUpdate", web_pixel_update)
        .mutation("webPixelDelete", web_pixel_delete)
        .query("webPixel", web_pixel)
        .mutation("serverPixelCreate", server_pixel_create)
        .mutation("serverPixelDelete", server_pixel_delete)
        .mutation(
            "eventBridgeServerPixelUpdate",
            event_bridge_server_pixel_update,
        )
        .mutation("pubSubServerPixelUpdate", pub_sub_server_pixel_update)
        .query("serverPixel", server_pixel)
        .mutation("storefrontAccessTokenCreate", storefront_token_create)
        .mutation("storefrontAccessTokenDelete", storefront_token_delete)
        .field("Shop", "storefrontAccessTokens", storefront_tokens);
}

// ---- script tags -------------------------------------------------------------------------

/// The `src` of the script tags the storefront loads, in creation order: those shown on the
/// online store (`ONLINE_STORE`, and the deprecated `ALL`). `ORDER_STATUS` is the order status
/// page's, which lsf's checkout does not render.
pub fn storefront_scripts(state: &ShopState) -> Vec<String> {
    state
        .all("ScriptTag")
        .filter(|tag| matches!(text(tag, "displayScope"), "ONLINE_STORE" | "ALL"))
        .map(|tag| text(tag, "src").to_string())
        .filter(|src| !src.is_empty())
        .collect()
}

/// Whether `src` is a URL a browser loads a script from.
fn is_script_url(src: &str) -> bool {
    let rest = src
        .strip_prefix("https://")
        .or_else(|| src.strip_prefix("http://"));
    rest.is_some_and(|rest| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        !host.is_empty() && !rest.contains(char::is_whitespace)
    })
}

/// The checks of a `ScriptTagInput`: `(src, displayScope, cache)`, the record's values for what
/// the input leaves out (an update).
fn check_script_tag(
    input: &Map<String, Value>,
    before: Option<&Record>,
    errors: &mut Vec<UserError>,
) -> (String, String, bool) {
    let src = match input.get("src").and_then(Value::as_str) {
        Some(src) => src.trim().to_string(),
        None => before.map_or_else(String::new, |record| text(record, "src").to_string()),
    };
    if src.is_empty() {
        errors.push(UserError::new(&["input", "src"], "Src can't be blank"));
    } else if !is_script_url(&src) {
        errors.push(UserError::new(&["input", "src"], "Src is invalid"));
    }
    let scope = match input.get("displayScope").and_then(Value::as_str) {
        // "`ORDER_STATUS` is deprecated and unavailable as a mutation input" (the schema).
        Some("ORDER_STATUS") => {
            errors.push(UserError::new(
                &["input", "displayScope"],
                "Display scope ORDER_STATUS is unavailable as a mutation input",
            ));
            "ORDER_STATUS".to_string()
        }
        Some(scope) => scope.to_string(),
        // REST's default was `all`, which GraphQL deprecates for `ONLINE_STORE`.
        None => before.map_or_else(
            || "ONLINE_STORE".to_string(),
            |record| text(record, "displayScope").to_string(),
        ),
    };
    let cache = input
        .get("cache")
        .and_then(Value::as_bool)
        .or_else(|| before.and_then(|record| record.get("cache")?.as_bool()))
        .unwrap_or(false);
    (src, scope, cache)
}

fn script_tag_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let mut errors = Vec::new();
    let (src, scope, cache) = check_script_tag(&input, None, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"scriptTag": null}), &errors));
    }
    let state = m.state_mut();
    let now = state.timestamp();
    let id = state.insert(value::object(json!({
        "__typename": "ScriptTag",
        "src": src,
        "displayScope": scope,
        "cache": cache,
        "createdAt": now,
        "updatedAt": now,
    })));
    state.emit("script_tags/create", &id);
    Ok(payload(json!({"scriptTag": id}), &[]))
}

fn script_tag_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let Some(record) = m.state().get_of(&id, "ScriptTag") else {
        let error = UserError::new(&["id"], "Script tag does not exist");
        return Ok(payload(json!({"scriptTag": null}), &[error]));
    };
    let input = ctx.arg_object_or_default("input");
    let mut errors = Vec::new();
    let (src, scope, cache) = check_script_tag(&input, Some(record), &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"scriptTag": null}), &errors));
    }
    let state = m.state_mut();
    if let Some(tag) = state.get_mut(&id) {
        tag.insert("src".into(), json!(src));
        tag.insert("displayScope".into(), json!(scope));
        tag.insert("cache".into(), json!(cache));
    }
    state.touch(&id);
    state.emit("script_tags/update", &id);
    Ok(payload(json!({"scriptTag": id}), &[]))
}

fn script_tag_delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if m.state().get_of(&id, "ScriptTag").is_none() {
        let error = UserError::new(&["id"], "Script tag does not exist");
        return Ok(payload(json!({"deletedScriptTagId": null}), &[error]));
    }
    m.state_mut().delete(&id, "script_tags/delete");
    Ok(payload(json!({"deletedScriptTagId": id}), &[]))
}

/// `scriptTags(src:)`: the script tags of that source only, then the generic connection.
fn script_tags(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let src = ctx.arg_str("src");
    let items: Vec<Value> = r
        .records_of("ScriptTag")
        .into_iter()
        .filter(|item| {
            src.is_none_or(|src| {
                r.item_map(item)
                    .is_some_and(|record| text(record, "src") == src)
            })
        })
        .collect();
    r.connection(ctx, items)
}

// ---- web pixel ---------------------------------------------------------------------------

/// The setting of the shop that holds the app's web pixel extension, as the seed read it from
/// `shopify.extension.toml`: `{handle, fields: {<key>: {type, validations}}}`.
pub const WEB_PIXEL_EXTENSION: &str = "webPixelExtension";

/// The settings of a `WebPixelInput`, as JSON: an object, or a string of JSON (the guide's
/// `settings: "{\"accountID\":\"123\"}"`).
fn settings_of(input: &Map<String, Value>) -> Result<Map<String, Value>, UserError> {
    let invalid = || {
        UserError::new(&["webPixel", "settings"], "Settings must be a JSON object")
            .code("INVALID_CONFIGURATION_JSON")
    };
    match input.get("settings") {
        Some(Value::Object(settings)) => Ok(settings.clone()),
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(settings)) => Ok(settings),
            _ => Err(invalid()),
        },
        _ => Err(invalid()),
    }
}

/// Checks web pixel settings against the extension's definition, as Shopify does. lsf's
/// wording but for "Missing keys: …", the reference's.
fn check_settings(
    state: &ShopState,
    extension: &Value,
    settings: &Map<String, Value>,
) -> Vec<UserError> {
    let no_fields = Map::new();
    let fields = extension
        .get("fields")
        .and_then(Value::as_object)
        .unwrap_or(&no_fields);
    let mut errors = Vec::new();
    let missing: Vec<&str> = fields
        .keys()
        .map(String::as_str)
        .filter(|key| !settings.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        errors.push(
            UserError::new(
                &["webPixel"],
                format!("Missing keys: {}", missing.join(", ")),
            )
            .code("INVALID_SETTINGS"),
        );
    }
    let unknown: Vec<&str> = settings
        .keys()
        .map(String::as_str)
        .filter(|key| !fields.contains_key(*key))
        .collect();
    if !unknown.is_empty() {
        errors.push(
            UserError::new(
                &["webPixel"],
                format!("Unknown keys: {}", unknown.join(", ")),
            )
            .code("INVALID_SETTINGS"),
        );
    }
    let currency = state.currency();
    for (key, field) in fields {
        let Some(given) = settings.get(key) else {
            continue;
        };
        let raw = match given {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        let kind = field
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("single_line_text_field");
        let validations = field
            .get("validations")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let cx = super::content::types::Context {
            state,
            currency: &currency,
            validations,
        };
        if let Err(invalid) = super::content::types::check(kind, &raw, &cx) {
            errors.push(
                UserError::new(&["webPixel"], format!("{key}: {}", invalid.message))
                    .code("INVALID_SETTINGS"),
            );
        }
    }
    errors
}

/// The settings checked against the app's web pixel extension, kept as Shopify answers them:
/// a string of compact JSON (the reference's `"{\"trackingId\":\"GA-TRACKING-ID-123\"}"`).
fn checked_settings(m: &Mutation<'_>, ctx: &FieldContext<'_>) -> Result<String, Vec<UserError>> {
    let state = m.state();
    let Some(extension) = state.setting(WEB_PIXEL_EXTENSION).cloned() else {
        return Err(vec![
            UserError::new(
                &[],
                "The app has no web pixel extension: add one (`type = \"web_pixel_extension\"` \
                 in extensions/<name>/shopify.extension.toml)",
            )
            .code("NO_EXTENSION"),
        ]);
    };
    let settings =
        settings_of(&ctx.arg_object_or_default("webPixel")).map_err(|error| vec![error])?;
    let errors = check_settings(state, &extension, &settings);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Value::Object(settings).to_string())
}

/// `webPixelCreate`: one web pixel per app.
fn web_pixel_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    if m.state().keys("WebPixel").next().is_some() {
        let error =
            UserError::new(&[], "Web pixel is taken: the app has one already").code("TAKEN");
        return Ok(payload(json!({"webPixel": null}), &[error]));
    }
    let settings = match checked_settings(m, ctx) {
        Ok(settings) => settings,
        Err(errors) => return Ok(payload(json!({"webPixel": null}), &errors)),
    };
    let state = m.state_mut();
    let id = state.insert(value::object(json!({
        "__typename": "WebPixel",
        "settings": settings,
    })));
    state.emit("web_pixels/create", &id);
    Ok(payload(json!({"webPixel": id}), &[]))
}

fn web_pixel_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if m.state().get_of(&id, "WebPixel").is_none() {
        let error = UserError::new(&["id"], "Web pixel not found").code("NOT_FOUND");
        return Ok(payload(json!({"webPixel": null}), &[error]));
    }
    let settings = match checked_settings(m, ctx) {
        Ok(settings) => settings,
        Err(errors) => return Ok(payload(json!({"webPixel": null}), &errors)),
    };
    let state = m.state_mut();
    if let Some(pixel) = state.get_mut(&id) {
        pixel.insert("settings".into(), json!(settings));
    }
    state.emit("web_pixels/update", &id);
    Ok(payload(json!({"webPixel": id}), &[]))
}

fn web_pixel_delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if m.state().get_of(&id, "WebPixel").is_none() {
        let error = UserError::new(&["id"], "Web pixel not found").code("NOT_FOUND");
        return Ok(payload(json!({"deletedWebPixelId": null}), &[error]));
    }
    m.state_mut().delete(&id, "web_pixels/delete");
    Ok(payload(json!({"deletedWebPixelId": id}), &[]))
}

/// `webPixel(id:)`: the one named, else the app's.
fn web_pixel(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let found = match ctx.arg_str("id") {
        Some(id) => r.reference(id, "WebPixel"),
        None => r
            .records_of("WebPixel")
            .first()
            .and_then(|item| item.as_str().and_then(|key| r.reference(key, "WebPixel"))),
    };
    Ok(found.map_or(Output::Null, Output::Object))
}

/// Removes the app's script tags and its web pixel, as uninstalling the app does. Answers how
/// many of each were removed.
///
/// As Shopify does: "Script tags are scoped to the app that created them. When an app is
/// uninstalled from a shop, all of the script tags that it created are automatically removed
/// along with it" (shopify.dev, `ScriptTag`); "all associated data and extensions, including web
/// pixel extensions, should be removed automatically" (community.shopify.com, "Should my app
/// delete the web pixel that it installed if the app is uninstalled?"), and a reinstalled app
/// creates its web pixel again (`webPixelCreate`). The storefront stops loading the script tags
/// at once: store sync gives the theme the script tags of the default shop.
pub fn remove_on_uninstall(state: &mut ShopState) -> (usize, usize) {
    let mut removed = (0, 0);
    for (type_name, count) in [("ScriptTag", &mut removed.0), ("WebPixel", &mut removed.1)] {
        let keys: Vec<String> = state.keys(type_name).map(str::to_string).collect();
        for key in keys {
            // No webhook: the app's subscriptions go with it.
            state.emit(super::app::CHANGED, &key);
            state.remove(&key);
            *count += 1;
        }
    }
    removed
}

// ---- server pixel ------------------------------------------------------------------------

fn server_pixel_key(state: &ShopState) -> Option<String> {
    state.keys("ServerPixel").next().map(str::to_string)
}

fn server_pixel(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(server_pixel_key(r.state())
        .and_then(|key| r.reference(&key, "ServerPixel"))
        .map_or(Output::Null, Output::Object))
}

/// `serverPixelCreate`: an unconfigured server pixel, one per app.
fn server_pixel_create(m: &mut Mutation<'_>, _ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    if server_pixel_key(m.state()).is_some() {
        let error = UserError::new(&[], "A server pixel already exists for this app and shop")
            .code("ALREADY_EXISTS");
        return Ok(payload(json!({"serverPixel": null}), &[error]));
    }
    let state = m.state_mut();
    let id = state.insert(value::object(json!({
        "__typename": "ServerPixel",
        "status": "DISCONNECTED_UNCONFIGURED",
        "webhookEndpointAddress": null,
    })));
    state.emit("server_pixels/create", &id);
    Ok(payload(json!({"serverPixel": id}), &[]))
}

fn server_pixel_delete(m: &mut Mutation<'_>, _ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let Some(id) = server_pixel_key(m.state()) else {
        let error = UserError::new(&[], "Server pixel not found").code("NOT_FOUND");
        return Ok(payload(json!({"deletedServerPixelId": null}), &[error]));
    };
    m.state_mut().delete(&id, "server_pixels/delete");
    Ok(payload(json!({"deletedServerPixelId": id}), &[]))
}

/// Connects the server pixel to an endpoint: `CONNECTED`, its address the endpoint's. "Running
/// this mutation deletes any previous subscriptions for the server pixel": the one address
/// replaces the last.
fn connect_server_pixel(
    m: &mut Mutation<'_>,
    address: Result<String, UserError>,
) -> Result<Value, FieldError> {
    let Some(id) = server_pixel_key(m.state()) else {
        let error = UserError::new(
            &[],
            "Server pixel not found: create it with serverPixelCreate",
        )
        .code("NOT_FOUND");
        return Ok(payload(json!({"serverPixel": null}), &[error]));
    };
    let address = match address {
        Ok(address) => address,
        Err(error) => return Ok(payload(json!({"serverPixel": null}), &[error])),
    };
    let state = m.state_mut();
    if let Some(pixel) = state.get_mut(&id) {
        pixel.insert("status".into(), json!("CONNECTED"));
        pixel.insert("webhookEndpointAddress".into(), json!(address));
    }
    state.emit("server_pixels/update", &id);
    m.note(format!(
        "{}: lsf records the endpoint and sends nothing to it; the storefront's customer events \
         are not collected",
        if address.starts_with("arn:") {
            "eventBridgeServerPixelUpdate"
        } else {
            "pubSubServerPixelUpdate"
        }
    ));
    Ok(payload(json!({"serverPixel": id}), &[]))
}

/// `eventBridgeServerPixelUpdate(arn:)`: an EventBridge partner event source, as a webhook
/// subscription's (`arn:aws:events:<region>::event-source/aws.partner/shopify.com/<id>/<name>`).
fn event_bridge_server_pixel_update(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let arn = ctx.arg_str("arn").unwrap_or_default().trim().to_string();
    let address = if arn.starts_with("arn:aws:events:") && arn.contains(":event-source/") {
        Ok(arn)
    } else {
        Err(UserError::new(
            &["arn"],
            "Arn is invalid: an EventBridge event source is arn:aws:events:<region>::event-source/aws.partner/shopify.com/<app id>/<name>",
        )
        .code("NEEDS_CONFIGURATION_TO_CONNECT"))
    };
    connect_server_pixel(m, address)
}

/// `pubSubServerPixelUpdate(pubSubProject:, pubSubTopic:)`: `pubsub://<project>:<topic>`, the
/// address of a Pub/Sub webhook subscription.
fn pub_sub_server_pixel_update(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let project = ctx.arg_str("pubSubProject").unwrap_or_default().trim();
    let topic = ctx.arg_str("pubSubTopic").unwrap_or_default().trim();
    let address = if project.is_empty() || topic.is_empty() {
        Err(UserError::new(
            &[if project.is_empty() {
                "pubSubProject"
            } else {
                "pubSubTopic"
            }],
            "The Pub/Sub project and topic can't be blank",
        )
        .code("PUB_SUB_ERROR"))
    } else {
        Ok(format!("pubsub://{project}:{topic}"))
    };
    connect_server_pixel(m, address)
}

// ---- storefront access tokens ------------------------------------------------------------

/// How many storefront access tokens a shop can have.
const MAX_STOREFRONT_TOKENS: usize = 100;

/// The scopes a storefront access token gets: the app's `unauthenticated_*` ones.
fn unauthenticated_scopes(state: &ShopState) -> Vec<Value> {
    state
        .all("AppInstallation")
        .next()
        .and_then(|installation| installation.get("accessScopes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|scope| {
            scope
                .get("handle")
                .and_then(Value::as_str)
                .is_some_and(|handle| handle.starts_with("unauthenticated_"))
        })
        .cloned()
        .collect()
}

fn storefront_token_create(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let title = input
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let mut errors = Vec::new();
    if title.is_empty() {
        errors.push(UserError::new(&["input", "title"], "Title can't be blank"));
    }
    if m.state().count("StorefrontAccessToken") >= MAX_STOREFRONT_TOKENS {
        errors.push(UserError::new(
            &[],
            format!(
                "A shop can have {MAX_STOREFRONT_TOKENS} active storefront access tokens at most"
            ),
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"storefrontAccessToken": null}), &errors));
    }
    let scopes = unauthenticated_scopes(m.state());
    let state = m.state_mut();
    let id = state.new_gid("StorefrontAccessToken");
    // Made of the shop and the id: the same run gives the same tokens, and no two are alike.
    let token =
        value::uuid(&format!("storefront-access-token/{}/{id}", state.domain())).replace('-', "");
    let now = state.timestamp();
    state.insert(value::object(json!({
        "__typename": "StorefrontAccessToken",
        "id": id,
        "title": title,
        "accessToken": token,
        "accessScopes": scopes,
        "createdAt": now,
        "updatedAt": now,
    })));
    state.emit("storefront_access_tokens/create", &id);
    if scopes.is_empty() {
        m.note(
            "storefrontAccessTokenCreate: the app has no unauthenticated_* scope, so the token has \
             none (add them to [access_scopes] of shopify.app.toml)",
        );
    }
    Ok(payload(json!({"storefrontAccessToken": id}), &[]))
}

fn storefront_token_delete(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx
        .arg_object_or_default("input")
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if m.state().get_of(&id, "StorefrontAccessToken").is_none() {
        let error = UserError::new(&["id"], "Storefront access token does not exist");
        return Ok(payload(
            json!({"deletedStorefrontAccessTokenId": null}),
            &[error],
        ));
    }
    m.state_mut().delete(&id, "storefront_access_tokens/delete");
    Ok(payload(json!({"deletedStorefrontAccessTokenId": id}), &[]))
}

/// `Shop.storefrontAccessTokens`: the tokens of the app on the shop.
fn storefront_tokens(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, r.records_of("StorefrontAccessToken"))
}

#[cfg(test)]
pub(crate) mod tests;
