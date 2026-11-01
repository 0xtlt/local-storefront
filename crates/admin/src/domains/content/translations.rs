//! Translations: `translationsRegister`, `translationsRemove`, the `translatableResource`,
//! `translatableResources` and `translatableResourcesByIds` queries, and the `translations` of
//! the resources that have them.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A resource's translatable content is a list of keys with their value, a digest and the
//!   shop's primary locale: `title`, `body_html`, `handle`, `product_type` of a product (the
//!   example of `translatableResources`), `title`, `body_html`, `handle` of a collection (the
//!   example of `translatableResource`), and the same of pages, blogs (no body) and articles
//!   (`summary_html` too); the `value` of a metafield of a translatable type; the fields of a
//!   metaobject of a translatable type; the `body` of a policy; the `name` of a product option and
//!   of an option value; the `title` of a menu. The digest is the SHA-256 of the value, in hex
//!   (as the examples show: "Featured items" is `a18b3403…`).
//! - `translationsRegister` takes a translation per key and locale (and market): the locale must
//!   be one of the shop's other than its primary one (`INVALID_LOCALE_FOR_SHOP`), the key one of
//!   the resource's (`INVALID_KEY_FOR_MODEL`), the digest that of the content now
//!   (`INVALID_TRANSLATABLE_CONTENT`, "Translatable content hash is invalid", Shopify's message as
//!   community answers quote it), the value not blank. A translation of the same key, locale and
//!   market is replaced.
//! - `translationsRemove` removes the translations of the keys and locales (and markets) given.
//! - A translation is `outdated` when the resource changed after it was registered: Shopify says
//!   so when the content of its key changed; lsf has the date of the resource, not of each key.
//!
//! Translations are kept in a record of their own per resource: a `TranslatableResource` (its
//! `resourceId`, its `translations`), under the key `translations/<resource GID>`. They are not
//! store data: the storefront does not show them. Shopify has no webhook topic for them: their
//! change is the event `translations/update` on the resource, which no subscription can name.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::metafields::{is_translatable, text};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .query("translatableResource", translatable_resource)
        .query("translatableResources", translatable_resources)
        .query("translatableResourcesByIds", translatable_resources_by_ids)
        .field(
            "TranslatableResource",
            "translatableContent",
            translatable_content,
        )
        .field(
            "TranslatableResource",
            "translations",
            resource_translations,
        )
        .field(
            "TranslatableResource",
            "nestedTranslatableResources",
            nested,
        )
        .mutation("translationsRegister", register_translations)
        .mutation("translationsRemove", remove_translations);
    for type_name in TRANSLATED_TYPES {
        registry.field(type_name, "translations", record_translations);
    }
}

/// The record types whose content lsf translates, and their `TranslatableResourceType`.
const RESOURCE_TYPES: &[(&str, &str)] = &[
    ("Product", "PRODUCT"),
    ("Collection", "COLLECTION"),
    ("Page", "PAGE"),
    ("Blog", "BLOG"),
    ("Article", "ARTICLE"),
    ("Metafield", "METAFIELD"),
    ("Metaobject", "METAOBJECT"),
    ("ShopPolicy", "SHOP_POLICY"),
    ("ProductOption", "PRODUCT_OPTION"),
    ("ProductOptionValue", "PRODUCT_OPTION_VALUE"),
    ("Menu", "MENU"),
];

/// The types of `RESOURCE_TYPES` that have a `translations(locale:)` field of their own
/// (`HasPublishedTranslations`).
const TRANSLATED_TYPES: &[&str] = &[
    "Product",
    "Collection",
    "Page",
    "Blog",
    "Article",
    "Metafield",
    "ShopPolicy",
    "ProductOption",
    "ProductOptionValue",
    "Menu",
];

/// The digest of a translatable value: its SHA-256, in hex.
pub fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The `LocalizableContentType` of a metafield type.
fn content_type_of_metafield(kind: &str) -> &'static str {
    match kind {
        "single_line_text_field" => "SINGLE_LINE_TEXT_FIELD",
        "multi_line_text_field" => "MULTI_LINE_TEXT_FIELD",
        "rich_text_field" => "RICH_TEXT_FIELD",
        "json" => "JSON",
        "url" => "URL",
        "link" => "LINK",
        "file_reference" => "FILE_REFERENCE",
        _ => "STRING",
    }
}

/// The translatable content of a record: `(key, value, LocalizableContentType)`. `None` for a
/// record lsf does not translate.
fn content(state: &ShopState, key: &str) -> Option<Vec<(String, String, &'static str)>> {
    let record = state.get(key)?;
    let field = |name: &str| text(record, name).to_string();
    let entry = |key: &str, value: String, kind: &'static str| (key.to_string(), value, kind);
    Some(match state.type_of(key)? {
        "Product" => vec![
            entry("title", field("title"), "SINGLE_LINE_TEXT_FIELD"),
            entry("body_html", field("descriptionHtml"), "HTML"),
            entry("handle", field("handle"), "URI"),
            entry(
                "product_type",
                field("productType"),
                "SINGLE_LINE_TEXT_FIELD",
            ),
        ],
        "Collection" => vec![
            entry("title", field("title"), "SINGLE_LINE_TEXT_FIELD"),
            entry("body_html", field("descriptionHtml"), "HTML"),
            entry("handle", field("handle"), "URI"),
        ],
        "Page" => vec![
            entry("title", field("title"), "SINGLE_LINE_TEXT_FIELD"),
            entry("body_html", field("body"), "HTML"),
            entry("handle", field("handle"), "URI"),
        ],
        "Blog" => vec![
            entry("title", field("title"), "SINGLE_LINE_TEXT_FIELD"),
            entry("handle", field("handle"), "URI"),
        ],
        "Article" => vec![
            entry("title", field("title"), "SINGLE_LINE_TEXT_FIELD"),
            entry("body_html", field("body"), "HTML"),
            entry("summary_html", field("summary"), "HTML"),
            entry("handle", field("handle"), "URI"),
        ],
        "Metafield" => {
            let kind = text(record, "type");
            if !is_translatable(kind) {
                return Some(Vec::new());
            }
            vec![entry(
                "value",
                field("value"),
                content_type_of_metafield(kind),
            )]
        }
        "Metaobject" => record
            .get("fields")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|metaobject_field| {
                let kind = metaobject_field.get("type")?.as_str()?;
                let value = metaobject_field.get("value")?.as_str()?;
                let key = metaobject_field.get("key")?.as_str()?;
                is_translatable(kind)
                    .then(|| entry(key, value.to_string(), content_type_of_metafield(kind)))
            })
            .collect(),
        "ShopPolicy" => vec![entry("body", field("body"), "HTML")],
        "ProductOption" | "ProductOptionValue" => vec![entry("name", field("name"), "STRING")],
        "Menu" => vec![entry("title", field("title"), "SINGLE_LINE_TEXT_FIELD")],
        _ => return None,
    })
}

/// The shop's primary locale.
fn primary_locale(state: &ShopState) -> String {
    state
        .all("ShopLocale")
        .find(|locale| locale.get("primary").and_then(Value::as_bool) == Some(true))
        .map_or_else(
            || "en".to_string(),
            |locale| text(locale, "locale").to_string(),
        )
}

/// The key of the record that keeps a resource's translations.
fn translations_key(resource: &str) -> String {
    format!("translations/{resource}")
}

/// The translations of a resource, as kept.
fn translations_of(state: &ShopState, resource: &str) -> Vec<Value> {
    state
        .get(&translations_key(resource))
        .and_then(|record| record.get("translations"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The translations of a resource for a locale (and a market: none, the shop's), with
/// `outdated` (the resource changed after the translation) and `outdated:` applied.
fn translations_for(
    state: &ShopState,
    resource: &str,
    locale: &str,
    market: Option<&str>,
    outdated: Option<bool>,
) -> Vec<Value> {
    let changed = state
        .get(resource)
        .and_then(|record| record.get("updatedAt"))
        .and_then(Value::as_str)
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok());
    translations_of(state, resource)
        .into_iter()
        .filter(|translation| {
            translation.get("locale").and_then(Value::as_str) == Some(locale)
                && translation.get("market").and_then(Value::as_str) == market
        })
        .map(|mut translation| {
            let registered = translation
                .get("updatedAt")
                .and_then(Value::as_str)
                .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok());
            let is_outdated =
                matches!((changed, registered), (Some(changed), Some(at)) if changed > at);
            translation["outdated"] = json!(is_outdated);
            translation
        })
        .filter(|translation| {
            outdated.is_none_or(|wanted| translation["outdated"].as_bool() == Some(wanted))
        })
        .collect()
}

/// A `TranslatableResource` object of a resource.
fn resource_object(resource: &str) -> Obj {
    let mut value = Record::new();
    value.insert("resourceId".into(), json!(resource));
    Obj::built("TranslatableResource", value)
}

/// The resource a `TranslatableResource` object is about.
fn resource_of<'o>(r: &'o ShopResolver<'_>, object: &'o Obj) -> String {
    r.map_of(object)
        .map(|record| text(record, "resourceId").to_string())
        .unwrap_or_default()
}

fn translatable_resource(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let resource = ctx.arg_str("resourceId").unwrap_or_default();
    Ok(Output::object_or_null(
        content(r.state(), resource).map(|_| resource_object(resource)),
    ))
}

/// The resources of a `TranslatableResourceType`, as `{resourceId}` objects.
fn resources_of_type(r: &ShopResolver<'_>, resource_type: &str) -> Vec<Value> {
    let Some((type_name, _)) = RESOURCE_TYPES
        .iter()
        .find(|(_, kind)| *kind == resource_type)
    else {
        return Vec::new();
    };
    r.state()
        .keys(type_name)
        .filter(|key| {
            // A metafield of a type that is not translatable is not a translatable resource.
            content(r.state(), key).is_some_and(|content| !content.is_empty())
        })
        .map(|key| json!({"resourceId": key}))
        .collect()
}

fn translatable_resources(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let resource_type = ctx.arg_str("resourceType").unwrap_or_default();
    if !RESOURCE_TYPES
        .iter()
        .any(|(_, kind)| *kind == resource_type)
    {
        r.note(format!(
            "translatableResources: lsf does not translate {resource_type} resources: the list is empty"
        ));
    }
    r.connection(ctx, resources_of_type(r, resource_type))
}

fn translatable_resources_by_ids(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = ctx
        .arg("resourceIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|id| content(r.state(), id).is_some())
        .map(|id| json!({"resourceId": id}))
        .collect();
    r.connection(ctx, items)
}

fn translatable_content(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let resource = resource_of(r, object);
    let locale = primary_locale(r.state());
    let entries = content(r.state(), &resource).unwrap_or_default();
    Ok(Output::List(
        entries
            .into_iter()
            .map(|(key, value, kind)| {
                let item = value::object(json!({
                    "key": key,
                    "digest": digest(&value),
                    "value": value,
                    "locale": locale,
                    "type": kind,
                }));
                Output::Object(Obj::built("TranslatableContent", item))
            })
            .collect(),
    ))
}

/// The translations of a resource, as `Translation` objects, from the field's `locale`,
/// `marketId` and `outdated`.
fn translation_outputs(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    resource: &str,
) -> Output<Obj> {
    let locale = ctx.arg_str("locale").unwrap_or_default();
    let translations = translations_for(
        r.state(),
        resource,
        locale,
        ctx.arg_str("marketId"),
        ctx.arg_bool("outdated"),
    );
    Output::List(
        translations
            .into_iter()
            .filter_map(|translation| match translation {
                Value::Object(map) => Some(Output::Object(Obj::built("Translation", map))),
                _ => None,
            })
            .collect(),
    )
}

fn resource_translations(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let resource = resource_of(r, object);
    Ok(translation_outputs(r, ctx, &resource))
}

/// `translations(locale:)` of a translated record (`Product.translations`).
fn record_translations(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    match object.key() {
        Some(key) => Ok(translation_outputs(r, ctx, key)),
        None => r.generic(ctx, object),
    }
}

/// `nestedTranslatableResources`: the translatable metafields of a resource, and the options of
/// a product.
fn nested(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let resource = resource_of(r, object);
    let state = r.state();
    let Some(record) = state.get(&resource) else {
        return r.connection(ctx, Vec::new());
    };
    let wanted = ctx.arg_str("resourceType");
    let mut items = Vec::new();
    for (field, kind) in [("metafields", "METAFIELD"), ("options", "PRODUCT_OPTION")] {
        if wanted.is_some_and(|wanted| wanted != kind) {
            continue;
        }
        for id in record
            .get(field)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if content(state, id).is_some_and(|content| !content.is_empty()) {
                items.push(json!({"resourceId": id}));
            }
        }
    }
    r.connection(ctx, items)
}

/// The locales of the shop a translation may be in: its locales other than the primary one.
fn translation_locales(state: &ShopState) -> Vec<String> {
    state
        .all("ShopLocale")
        .filter(|locale| locale.get("primary").and_then(Value::as_bool) != Some(true))
        .map(|locale| text(locale, "locale").to_string())
        .collect()
}

/// `translationsRegister`.
fn register_translations(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let resource = ctx.arg_str("resourceId").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(entries) = content(state, &resource) else {
        let error = UserError::new(
            &["resourceId"],
            format!("Resource {resource} does not exist"),
        )
        .code("RESOURCE_NOT_FOUND");
        return Ok(payload(json!({"translations": null}), &[error]));
    };
    let locales = translation_locales(state);
    let inputs: Vec<Map<String, Value>> = ctx
        .arg("translations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|input| input.as_object().cloned())
        .collect();
    let mut errors = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let index = index.to_string();
        let get = |name: &str| input.get(name).and_then(Value::as_str).unwrap_or_default();
        let at = |field: &str| vec!["translations".to_string(), index.clone(), field.to_string()];
        let error = |field: &str, message: String, code: &str| UserError {
            field: Some(at(field)),
            message,
            code: Some(code.to_string()),
        };
        if !locales.iter().any(|locale| locale == get("locale")) {
            errors.push(error(
                "locale",
                format!("Locale {} is not enabled for this shop", get("locale")),
                "INVALID_LOCALE_FOR_SHOP",
            ));
        }
        match entries.iter().find(|(key, _, _)| key == get("key")) {
            None => errors.push(error(
                "key",
                format!("Key {} is not a valid translatable field", get("key")),
                "INVALID_KEY_FOR_MODEL",
            )),
            Some((_, value, _)) if digest(value) != get("translatableContentDigest") => errors
                .push(error(
                    "translatableContentDigest",
                    "Translatable content hash is invalid".to_string(),
                    "INVALID_TRANSLATABLE_CONTENT",
                )),
            Some(_) => {}
        }
        if get("value").trim().is_empty() {
            errors.push(error("value", "Value can't be blank".to_string(), "BLANK"));
        }
        if let Some(market) = input.get("marketId").and_then(Value::as_str)
            && state.type_of(market) != Some("Market")
        {
            errors.push(error(
                "marketId",
                format!("Market {market} does not exist"),
                "MARKET_DOES_NOT_EXIST",
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"translations": null}), &errors));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let mut kept = translations_of(state, &resource);
    let mut registered = Vec::new();
    for input in &inputs {
        let get = |name: &str| input.get(name).and_then(Value::as_str);
        let translation = json!({
            "key": get("key"),
            "value": get("value"),
            "locale": get("locale"),
            "market": get("marketId"),
            "outdated": false,
            "updatedAt": now,
        });
        kept.retain(|known| {
            !(known.get("key") == translation.get("key")
                && known.get("locale") == translation.get("locale")
                && known.get("market") == translation.get("market"))
        });
        kept.push(translation.clone());
        registered.push(translation);
    }
    store(state, &resource, kept);
    state.emit("translations/update", &resource);
    Ok(payload(json!({"translations": registered}), &[]))
}

/// Keeps the translations of a resource (none: its record goes).
fn store(state: &mut ShopState, resource: &str, translations: Vec<Value>) {
    let key = translations_key(resource);
    if translations.is_empty() {
        state.remove(&key);
        return;
    }
    let record = value::object(json!({
        "__typename": "TranslatableResource",
        "resourceId": resource,
        "translations": translations,
    }));
    state.insert_as(key, record);
}

/// `translationsRemove`.
fn remove_translations(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let resource = ctx.arg_str("resourceId").unwrap_or_default().to_string();
    let state = mutation.state();
    if content(state, &resource).is_none() {
        let error = UserError::new(
            &["resourceId"],
            format!("Resource {resource} does not exist"),
        )
        .code("RESOURCE_NOT_FOUND");
        return Ok(payload(json!({"translations": null}), &[error]));
    }
    let strings = |name: &str| -> Vec<String> {
        ctx.arg(name)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    };
    let (keys, locales, markets) = (
        strings("translationKeys"),
        strings("locales"),
        ctx.arg("marketIds").map(|_| strings("marketIds")),
    );
    let locales_known = translation_locales(state);
    let errors: Vec<UserError> = locales
        .iter()
        .enumerate()
        .filter(|(_, locale)| !locales_known.contains(locale))
        .map(|(index, locale)| {
            UserError::new(
                &["locales", &index.to_string()],
                format!("Locale {locale} is not enabled for this shop"),
            )
            .code("INVALID_LOCALE_FOR_SHOP")
        })
        .collect();
    if !errors.is_empty() {
        return Ok(payload(json!({"translations": null}), &errors));
    }
    let matches = |translation: &Value| {
        let field = |name: &str| translation.get(name).and_then(Value::as_str);
        field("key").is_some_and(|key| keys.iter().any(|wanted| wanted == key))
            && field("locale").is_some_and(|locale| locales.iter().any(|wanted| wanted == locale))
            && match &markets {
                Some(markets) => field("market")
                    .is_some_and(|market| markets.iter().any(|wanted| wanted == market)),
                None => field("market").is_none(),
            }
    };
    let kept = translations_of(state, &resource);
    let removed: Vec<Value> = kept
        .iter()
        .filter(|translation| matches(translation))
        .cloned()
        .collect();
    if removed.is_empty() {
        return Ok(payload(json!({"translations": []}), &[]));
    }
    let remaining: Vec<Value> = kept
        .into_iter()
        .filter(|translation| !matches(translation))
        .collect();
    let state = mutation.state_mut();
    store(state, &resource, remaining);
    state.emit("translations/update", &resource);
    Ok(payload(json!({"translations": removed}), &[]))
}
