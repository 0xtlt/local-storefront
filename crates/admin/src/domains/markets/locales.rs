//! The shop's locales: `shopLocaleEnable`, `shopLocaleUpdate`, `shopLocaleDisable`, and the
//! `shopLocales(published:)` and `availableLocales` queries.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `shopLocaleEnable` "adds a locale for a shop. The newly added locale is in the unpublished
//!   state" (its example answers `{"locale": "ko", "name": "Korean", "primary": false,
//!   "published": false}`); `marketWebPresenceIds` makes it an alternate locale of those web
//!   presences.
//! - `shopLocaleUpdate` publishes or unpublishes it, and sets the web presences that have it.
//! - `shopLocaleDisable` "deletes a locale for a shop. This also deletes all translations of
//!   this locale": its translations go, and the web presences lose it.
//! - `availableLocales` lists the locales a shop can enable: Shopify's 137, with their names, as
//!   the example of the query answers them (`AVAILABLE_LOCALES`).
//! - Events: `locales/create`, `locales/update`, `locales/destroy`, whose payload is the locale
//!   and whether it is published.
//!
//! The errors are `UserError`s without a code; their messages are lsf's (shopify.dev publishes
//! none). A published locale reaches the storefront, which serves it under `/<locale>/` (see
//! "Store sync" in `crates/admin/docs/design.md`).

use serde_json::{Value, json};

use super::{strings, web_presences};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, input_path, payload};
use crate::state::{ShopState, value};

/// The locales a shop can enable, as `availableLocales` answers them on Shopify (the example
/// of the query in the 2026-10 reference), in its order: by English name.
pub const AVAILABLE_LOCALES: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("ak", "Akan"),
    ("sq", "Albanian"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("hy", "Armenian"),
    ("as", "Assamese"),
    ("az", "Azerbaijani"),
    ("bm", "Bambara"),
    ("bn", "Bangla"),
    ("eu", "Basque"),
    ("be", "Belarusian"),
    ("bs", "Bosnian"),
    ("br", "Breton"),
    ("bg", "Bulgarian"),
    ("my", "Burmese"),
    ("ca", "Catalan"),
    ("ckb", "Central Kurdish"),
    ("ce", "Chechen"),
    ("zh-CN", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"),
    ("kw", "Cornish"),
    ("hr", "Croatian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("nl", "Dutch"),
    ("dz", "Dzongkha"),
    ("en", "English"),
    ("eo", "Esperanto"),
    ("et", "Estonian"),
    ("ee", "Ewe"),
    ("fo", "Faroese"),
    ("fil", "Filipino"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("ff", "Fulah"),
    ("gl", "Galician"),
    ("lg", "Ganda"),
    ("ka", "Georgian"),
    ("de", "German"),
    ("el", "Greek"),
    ("gu", "Gujarati"),
    ("ha", "Hausa"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hu", "Hungarian"),
    ("is", "Icelandic"),
    ("ig", "Igbo"),
    ("id", "Indonesian"),
    ("ia", "Interlingua"),
    ("ga", "Irish"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("jv", "Javanese"),
    ("kl", "Kalaallisut"),
    ("kn", "Kannada"),
    ("ks", "Kashmiri"),
    ("kk", "Kazakh"),
    ("km", "Khmer"),
    ("ki", "Kikuyu"),
    ("rw", "Kinyarwanda"),
    ("ko", "Korean"),
    ("ku", "Kurdish"),
    ("ky", "Kyrgyz"),
    ("lo", "Lao"),
    ("lv", "Latvian"),
    ("ln", "Lingala"),
    ("lt", "Lithuanian"),
    ("lu", "Luba-Katanga"),
    ("lb", "Luxembourgish"),
    ("mk", "Macedonian"),
    ("mg", "Malagasy"),
    ("ms", "Malay"),
    ("ml", "Malayalam"),
    ("mt", "Maltese"),
    ("gv", "Manx"),
    ("mr", "Marathi"),
    ("mn", "Mongolian"),
    ("mi", "Māori"),
    ("ne", "Nepali"),
    ("nd", "North Ndebele"),
    ("se", "Northern Sami"),
    ("no", "Norwegian"),
    ("nb", "Norwegian (Bokmål)"),
    ("nn", "Norwegian Nynorsk"),
    ("or", "Odia"),
    ("om", "Oromo"),
    ("os", "Ossetic"),
    ("ps", "Pashto"),
    ("fa", "Persian"),
    ("pl", "Polish"),
    ("pt-BR", "Portuguese (Brazil)"),
    ("pt-PT", "Portuguese (Portugal)"),
    ("pa", "Punjabi"),
    ("qu", "Quechua"),
    ("ro", "Romanian"),
    ("rm", "Romansh"),
    ("rn", "Rundi"),
    ("ru", "Russian"),
    ("sg", "Sango"),
    ("sa", "Sanskrit"),
    ("sc", "Sardinian"),
    ("gd", "Scottish Gaelic"),
    ("sr", "Serbian"),
    ("sn", "Shona"),
    ("ii", "Sichuan Yi"),
    ("sd", "Sindhi"),
    ("si", "Sinhala"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("so", "Somali"),
    ("es", "Spanish"),
    ("su", "Sundanese"),
    ("sw", "Swahili"),
    ("sv", "Swedish"),
    ("tg", "Tajik"),
    ("ta", "Tamil"),
    ("tt", "Tatar"),
    ("te", "Telugu"),
    ("th", "Thai"),
    ("bo", "Tibetan"),
    ("ti", "Tigrinya"),
    ("to", "Tongan"),
    ("tr", "Turkish"),
    ("tk", "Turkmen"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("ug", "Uyghur"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("cy", "Welsh"),
    ("fy", "Western Frisian"),
    ("wo", "Wolof"),
    ("xh", "Xhosa"),
    ("yi", "Yiddish"),
    ("yo", "Yoruba"),
    ("zu", "Zulu"),
];

pub fn register(registry: &mut Registry) {
    registry
        .mutation("shopLocaleEnable", enable)
        .mutation("shopLocaleUpdate", update)
        .mutation("shopLocaleDisable", disable)
        .query("shopLocales", shop_locales)
        .query("availableLocales", available_locales);
}

/// The name of an available locale, matched without case (`pt-br` is `pt-BR`).
fn available(locale: &str) -> Option<(&'static str, &'static str)> {
    AVAILABLE_LOCALES
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(locale))
        .copied()
}

/// `shopLocales(published:)`: the shop's locales, the primary first; only the published ones
/// with `published: true`.
fn shop_locales(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let only_published = ctx.arg_bool("published") == Some(true);
    Ok(Output::objects(
        super::shop_locales(state)
            .into_iter()
            .filter(|locale| {
                !only_published
                    || state
                        .get(locale)
                        .and_then(|record| record.get("published"))
                        .and_then(Value::as_bool)
                        == Some(true)
            })
            .map(|locale| Obj::record(&locale)),
    ))
}

fn available_locales(
    _r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(Output::objects(AVAILABLE_LOCALES.iter().map(
        |(code, name)| {
            Obj::built(
                "Locale",
                value::object(json!({"isoCode": code, "name": name})),
            )
        },
    )))
}

/// The key of an enabled locale (matched without case).
fn enabled(state: &ShopState, locale: &str) -> Option<String> {
    super::shop_locales(state)
        .into_iter()
        .find(|known| known.eq_ignore_ascii_case(locale))
}

/// The web presences of `marketWebPresenceIds`, or the errors.
fn presences(
    state: &ShopState,
    ids: Option<&Value>,
    at: &[&str],
    errors: &mut Vec<UserError>,
) -> Option<Vec<String>> {
    let ids = ids.filter(|ids| !ids.is_null())?;
    let ids = strings(Some(ids));
    for (index, id) in ids.iter().enumerate() {
        if state.type_of(id) != Some("MarketWebPresence") {
            let index = index.to_string();
            let field = input_path(at, &[&index]);
            errors.push(UserError::new(&field, "Web presence does not exist"));
        }
    }
    Some(ids)
}

/// `shopLocaleEnable`.
fn enable(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let asked = ctx.arg_str("locale").unwrap_or_default();
    let mut errors = Vec::new();
    let Some((locale, name)) = available(asked) else {
        let error = UserError::new(&["locale"], format!("Locale {asked} is not available"));
        return Ok(payload(json!({"shopLocale": null}), &[error]));
    };
    if enabled(state, locale).is_some() {
        errors.push(UserError::new(
            &["locale"],
            format!("Locale {locale} is already enabled"),
        ));
    }
    let presences = presences(
        state,
        ctx.arg("marketWebPresenceIds"),
        &["marketWebPresenceIds"],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"shopLocale": null}), &errors));
    }
    let state = mutation.state_mut();
    state.insert_as(
        locale,
        value::object(json!({
            "__typename": "ShopLocale",
            "locale": locale,
            "name": name,
            "primary": false,
            "published": false,
            "marketWebPresences": [],
        })),
    );
    if let Some(presences) = presences {
        web_presences::set_locale_presences(state, locale, &presences);
    }
    state.emit("locales/create", locale);
    Ok(payload(json!({"shopLocale": locale}), &[]))
}

/// `shopLocaleUpdate`.
fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let asked = ctx.arg_str("locale").unwrap_or_default();
    let Some(locale) = enabled(state, asked) else {
        let error = UserError::new(&["locale"], format!("Locale {asked} is not enabled"));
        return Ok(payload(json!({"shopLocale": null}), &[error]));
    };
    let input = ctx.arg_object_or_default("shopLocale");
    let mut errors = Vec::new();
    let primary = state
        .get(&locale)
        .and_then(|record| record.get("primary"))
        .and_then(Value::as_bool)
        == Some(true);
    let published = input.get("published").and_then(Value::as_bool);
    if primary && published == Some(false) {
        errors.push(UserError::new(
            &["shopLocale", "published"],
            "The primary locale cannot be unpublished",
        ));
    }
    let presences = presences(
        state,
        input.get("marketWebPresenceIds"),
        &["shopLocale", "marketWebPresenceIds"],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"shopLocale": null}), &errors));
    }
    let state = mutation.state_mut();
    if let Some(published) = published
        && let Some(record) = state.get_mut(&locale)
    {
        record.insert("published".into(), json!(published));
    }
    if let Some(presences) = presences {
        web_presences::set_locale_presences(state, &locale, &presences);
    }
    state.emit("locales/update", &locale);
    Ok(payload(json!({"shopLocale": locale}), &[]))
}

/// `shopLocaleDisable`: the locale goes, with its translations and its place in the web
/// presences.
fn disable(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let asked = ctx.arg_str("locale").unwrap_or_default();
    let Some(locale) = enabled(state, asked) else {
        let error = UserError::new(&["locale"], format!("Locale {asked} is not enabled"));
        return Ok(payload(json!({"locale": null}), &[error]));
    };
    let record = state.get(&locale).cloned().unwrap_or_default();
    if record.get("primary").and_then(Value::as_bool) == Some(true) {
        let error = UserError::new(&["locale"], "The primary locale cannot be disabled");
        return Ok(payload(json!({"locale": null}), &[error]));
    }
    if web_presences::is_default_locale(state, &locale) {
        let error = UserError::new(
            &["locale"],
            "The locale is the default locale of a web presence",
        );
        return Ok(payload(json!({"locale": null}), &[error]));
    }
    let translated: Vec<String> = state
        .keys("TranslatableResource")
        .filter(|key| {
            state
                .get(key)
                .and_then(|record| record.get("translations"))
                .and_then(Value::as_array)
                .is_some_and(|translations| {
                    translations
                        .iter()
                        .any(|translation| translation.get("locale") == Some(&json!(locale)))
                })
        })
        .map(str::to_string)
        .collect();
    let state = mutation.state_mut();
    for key in translated {
        let resource = state
            .get(&key)
            .and_then(|record| record.get("resourceId"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let left = match state
            .get_mut(&key)
            .and_then(|record| record.get_mut("translations"))
        {
            Some(Value::Array(translations)) => {
                translations
                    .retain(|translation| translation.get("locale") != Some(&json!(locale)));
                translations.len()
            }
            _ => 0,
        };
        if left == 0 {
            state.remove(&key);
        }
        if let Some(resource) = resource {
            // The content family's event of a change of translations.
            state.emit("translations/update", &resource);
        }
    }
    web_presences::remove_locale(state, &locale);
    state.emit_with("locales/destroy", &locale, record);
    state.remove(&locale);
    Ok(payload(json!({"locale": locale}), &[]))
}
