//! Web presences: `webPresenceCreate`, `webPresenceUpdate`, `webPresenceDelete`, the deprecated
//! `marketWebPresenceCreate`, `marketWebPresenceUpdate`, `marketWebPresenceDelete`, and the
//! fields that read them (`Market.webPresences`, `Market.webPresence`,
//! `ShopLocale.marketWebPresences`, `MarketWebPresence.market`).
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A web presence is a domain (`domainId`) or a subfolder of the primary domain
//!   (`subfolderSuffix`), with a default locale and alternate ones, which must be locales of the
//!   shop. Its `rootUrls` are a URL per locale: `https://<host>` for the default locale of a
//!   domain, `https://<host>/<locale>` for the others; `https://<primary host>/<locale>-<suffix>`
//!   for every locale of a subfolder (the examples of `markets`: `/en-ca`, `/fr-ca`).
//! - A web presence has a domain or a subfolder, not both (`REQUIRES_DOMAIN_OR_SUBFOLDER`,
//!   `CANNOT_HAVE_SUBFOLDER_AND_DOMAIN`); a subfolder suffix has two letters at least and is not
//!   a script code (`Latn`, `Hans`, …); a domain or a suffix is another presence's at most once
//!   (`TAKEN`). The primary domain's presence cannot be deleted
//!   (`SHOP_MUST_HAVE_PRIMARY_DOMAIN_WEB_PRESENCE`).
//! - Shopify has no topic for web presences: a domain whose presence or locales change sends
//!   `domains/update`, whose payload has the domain's `localization`.
//!
//! The storefront serves every locale under `/<locale>/` on its one address: web presences do
//! not reach it (see "Store sync" in `crates/admin/docs/design.md`).

use serde_json::{Map, Value, json};

use super::{market_not_found, shop_locales, strings, text};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, coded, payload};
use crate::state::{ShopState, gid_number, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("webPresenceCreate", create)
        .mutation("webPresenceUpdate", update)
        .mutation("webPresenceDelete", delete)
        .mutation("marketWebPresenceCreate", market_create)
        .mutation("marketWebPresenceUpdate", market_update)
        .mutation("marketWebPresenceDelete", market_delete)
        .field("Market", "webPresences", market_web_presences)
        .field("Market", "webPresence", market_web_presence)
        .field("ShopLocale", "marketWebPresences", locale_web_presences)
        .field("MarketWebPresence", "market", presence_market);
}

/// The web presences of a market, as GIDs.
pub fn of_market(state: &ShopState, market: &str) -> Vec<Value> {
    state
        .keys("MarketWebPresence")
        .filter(|presence| {
            state.get(presence).is_some_and(|record| {
                strings(record.get("markets"))
                    .iter()
                    .any(|known| known == market)
            })
        })
        .map(|presence| json!(presence))
        .collect()
}

fn market_web_presences(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let market = object.key().unwrap_or_default();
    r.connection(ctx, of_market(r.state(), market))
}

/// `Market.webPresence` (deprecated): the first of its web presences.
fn market_web_presence(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let market = object.key().unwrap_or_default();
    Ok(Output::object_or_null(
        of_market(r.state(), market)
            .first()
            .and_then(Value::as_str)
            .map(Obj::record),
    ))
}

/// `ShopLocale.marketWebPresences`: the presences whose default or alternate locales have it.
fn locale_web_presences(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let locale = r
        .map_of(object)
        .and_then(|record| record.get("locale"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(Output::objects(
        state
            .keys("MarketWebPresence")
            .filter(|presence| {
                state
                    .get(presence)
                    .is_some_and(|record| locales_of(record).iter().any(|known| known == locale))
            })
            .map(Obj::record),
    ))
}

/// `MarketWebPresence.market` (deprecated): its first market.
fn presence_market(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let first = r
        .map_of(object)
        .map(|record| strings(record.get("markets")))
        .unwrap_or_default()
        .into_iter()
        .find(|market| state.is_of(market, "Market"));
    Ok(Output::object_or_null(first.as_deref().map(Obj::record)))
}

/// The locales of a presence, the default first.
pub fn locales_of(record: &Map<String, Value>) -> Vec<String> {
    let mut locales: Vec<String> = record
        .get("defaultLocale")
        .and_then(Value::as_str)
        .map(str::to_string)
        .into_iter()
        .collect();
    for locale in strings(record.get("alternateLocales")) {
        if !locales.contains(&locale) {
            locales.push(locale);
        }
    }
    locales
}

/// The host of the shop's primary domain.
fn primary_host(state: &ShopState) -> String {
    state
        .shop()
        .and_then(|shop| shop.get("primaryDomain"))
        .and_then(Value::as_str)
        .and_then(|domain| state.get(domain))
        .and_then(|domain| domain.get("host"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| state.domain())
        .to_string()
}

/// The presence of the primary domain.
fn primary_presence(state: &ShopState) -> Option<String> {
    let domain = state
        .shop()
        .and_then(|shop| shop.get("primaryDomain"))
        .and_then(Value::as_str)?;
    presence_of_domain(state, domain)
}

/// The presence of a domain.
fn presence_of_domain(state: &ShopState, domain: &str) -> Option<String> {
    state
        .keys("MarketWebPresence")
        .find(|presence| {
            state
                .get(presence)
                .and_then(|record| record.get("domain"))
                .and_then(Value::as_str)
                == Some(domain)
        })
        .map(str::to_string)
}

/// Recomputes a presence's `rootUrls` from its domain or subfolder and its locales.
pub fn refresh(state: &mut ShopState, presence: &str) {
    let Some(record) = state.get(presence) else {
        return;
    };
    let locales = locales_of(record);
    let domain_host = record
        .get("domain")
        .and_then(Value::as_str)
        .and_then(|domain| state.get(domain))
        .and_then(|domain| domain.get("host"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let suffix = record
        .get("subfolderSuffix")
        .and_then(Value::as_str)
        .map(str::to_string);
    let primary = primary_host(state);
    let urls: Vec<Value> = locales
        .iter()
        .enumerate()
        .map(|(index, locale)| {
            let url = match (&domain_host, &suffix) {
                (_, Some(suffix)) => format!("https://{primary}/{locale}-{suffix}"),
                (Some(host), None) if index == 0 => format!("https://{host}"),
                (Some(host), None) => format!("https://{host}/{locale}"),
                (None, None) => format!("https://{primary}/{locale}"),
            };
            json!({"locale": locale, "url": url})
        })
        .collect();
    if state
        .get(presence)
        .and_then(|record| record.get("rootUrls"))
        != Some(&json!(urls))
        && let Some(record) = state.get_mut(presence)
    {
        record.insert("rootUrls".into(), json!(urls));
    }
}

/// The `localization` of a domain: its presence's locales (its country when the presence is
/// one country's), else the primary domain's presence's, which a domain without its own
/// redirects to.
fn localization_of(state: &ShopState, presence: Option<&str>) -> Value {
    let Some(record) = presence.and_then(|presence| state.get(presence)) else {
        return Value::Null;
    };
    let locales = locales_of(record);
    let countries: Vec<String> = strings(record.get("markets"))
        .iter()
        .filter_map(|market| state.get(market))
        .flat_map(super::regions_of)
        .collect();
    let country = (countries.len() == 1).then(|| countries[0].clone());
    json!({
        "defaultLocale": locales.first(),
        "alternateLocales": locales.iter().skip(1).collect::<Vec<_>>(),
        "country": country,
    })
}

/// Keeps every domain's `marketWebPresence` and `localization` right. The domains whose
/// presence or localization changed.
pub fn refresh_domains(state: &mut ShopState) -> Vec<String> {
    let domains: Vec<String> = state.keys("Domain").map(str::to_string).collect();
    let primary = primary_presence(state);
    let mut changed = Vec::new();
    for domain in domains {
        let own = presence_of_domain(state, &domain);
        let localization = localization_of(state, own.as_deref().or(primary.as_deref()));
        let record = state.get(&domain);
        let same = record.is_some_and(|record| {
            record.get("marketWebPresence") == Some(&json!(own))
                && record.get("localization") == Some(&localization)
        });
        if !same && let Some(record) = state.get_mut(&domain) {
            record.insert("marketWebPresence".into(), json!(own));
            record.insert("localization".into(), localization);
            changed.push(domain);
        }
    }
    changed
}

/// Refreshes the domains after a change of presences, and emits `domains/update` for each that
/// changed.
fn domains_changed(state: &mut ShopState) {
    for domain in refresh_domains(state) {
        state.emit("domains/update", &domain);
    }
}

/// Links presences to a market, or unlinks them.
pub fn link_market(state: &mut ShopState, market: &str, presences: &[String], linked: bool) {
    let mut touched = false;
    for presence in presences {
        let Some(record) = state.get(presence) else {
            continue;
        };
        let has = strings(record.get("markets"))
            .iter()
            .any(|known| known == market);
        if has == linked {
            continue;
        }
        if let Some(Value::Array(markets)) = state
            .get_mut(presence)
            .and_then(|record| record.get_mut("markets"))
        {
            markets.retain(|known| known.as_str() != Some(market));
            if linked {
                markets.push(json!(market));
            }
        }
        touched = true;
    }
    if touched {
        domains_changed(state);
    }
}

/// The four-letter script codes of ISO 15924 a subfolder suffix cannot be (`Latn`, …): those
/// of Shopify's locales.
const SCRIPT_CODES: &[&str] = &[
    "arab", "cyrl", "hans", "hant", "latn", "deva", "beng", "guru", "taml", "thai",
];

/// What a web presence input says, checked: its domain, suffix and locales.
struct Checked {
    domain: Option<String>,
    suffix: Option<String>,
    default: Option<String>,
    alternates: Option<Vec<String>>,
}

/// Checks a `WebPresenceCreateInput` or `…UpdateInput` (`at` its path) against the shop, for a
/// new presence or for `presence`.
fn check(
    state: &ShopState,
    input: &Map<String, Value>,
    at: &str,
    presence: Option<&str>,
    errors: &mut Vec<UserError>,
) -> Checked {
    let field = |name: &str| vec![at.to_string(), name.to_string()];
    let error = |name: &str, message: &str, code: &str| UserError {
        field: Some(field(name)),
        message: message.to_string(),
        code: Some(code.to_string()),
    };
    let existing = presence.and_then(|presence| state.get(presence));
    let domain = text(input, "domainId").map(str::to_string);
    let suffix = text(input, "subfolderSuffix").map(str::to_string);
    if let Some(domain) = &domain {
        if !state.is_of(domain, "Domain") {
            // lsf's wording, for the codes Shopify documents without a message.
            errors.push(error(
                "domainId",
                "Domain does not exist",
                "DOMAIN_NOT_FOUND",
            ));
        } else if presence_of_domain(state, domain)
            .is_some_and(|other| Some(other.as_str()) != presence)
        {
            errors.push(error("domainId", "Domain has already been taken", "TAKEN"));
        }
    }
    if let Some(suffix) = &suffix {
        if suffix.chars().count() < 2 || !suffix.chars().all(|c| c.is_ascii_alphabetic()) {
            errors.push(error(
                "subfolderSuffix",
                "Subfolder suffix must be at least 2 letters",
                "SUBFOLDER_SUFFIX_MUST_BE_AT_LEAST_2_LETTERS",
            ));
        } else if SCRIPT_CODES.contains(&suffix.to_ascii_lowercase().as_str()) {
            errors.push(error(
                "subfolderSuffix",
                "Subfolder suffix cannot be a script code",
                "SUBFOLDER_SUFFIX_CANNOT_BE_SCRIPT_CODE",
            ));
        } else if state.keys("MarketWebPresence").any(|other| {
            Some(other) != presence
                && state
                    .get(other)
                    .and_then(|record| record.get("subfolderSuffix"))
                    .and_then(Value::as_str)
                    .is_some_and(|known| known.eq_ignore_ascii_case(suffix))
        }) {
            errors.push(error(
                "subfolderSuffix",
                "Subfolder suffix has already been taken",
                "TAKEN",
            ));
        }
    }
    let has_domain = domain.is_some()
        || (suffix.is_none()
            && existing.is_some_and(|record| record.get("domain").is_some_and(|d| !d.is_null())));
    let has_suffix = suffix.is_some()
        || (domain.is_none()
            && existing
                .is_some_and(|record| record.get("subfolderSuffix").is_some_and(|s| !s.is_null())));
    if has_domain && has_suffix {
        errors.push(error(
            "subfolderSuffix",
            "A web presence cannot have both a domain and a subfolder",
            "CANNOT_HAVE_SUBFOLDER_AND_DOMAIN",
        ));
    } else if !has_domain && !has_suffix {
        errors.push(error(
            "domainId",
            "A web presence requires a domain or a subfolder",
            "REQUIRES_DOMAIN_OR_SUBFOLDER",
        ));
    }
    let locales = shop_locales(state);
    let default = text(input, "defaultLocale").map(str::to_string);
    if let Some(locale) = &default
        && !locales.contains(locale)
    {
        errors.push(error(
            "defaultLocale",
            "The language isn't enabled on the store",
            "DISABLED_LANGUAGE",
        ));
    }
    let alternates = input
        .get("alternateLocales")
        .filter(|value| !value.is_null())
        .map(|value| strings(Some(value)));
    if let Some(alternates) = &alternates {
        for (index, locale) in alternates.iter().enumerate() {
            let index = index.to_string();
            if !locales.contains(locale) {
                errors.push(UserError {
                    field: Some(vec![at.to_string(), "alternateLocales".into(), index]),
                    message: "The language isn't enabled on the store".into(),
                    code: Some("DISABLED_LANGUAGE".into()),
                });
            } else if alternates.iter().filter(|known| *known == locale).count() > 1
                || default.as_ref() == Some(locale)
            {
                errors.push(UserError {
                    field: Some(vec![at.to_string(), "alternateLocales".into(), index]),
                    message: "Languages are duplicated".into(),
                    code: Some("DUPLICATE_LANGUAGES".into()),
                });
            }
        }
    }
    Checked {
        domain,
        suffix,
        default,
        alternates,
    }
}

/// Creates a presence for markets. Its GID, or the errors.
fn create_presence(
    mutation: &mut Mutation<'_>,
    input: &Map<String, Value>,
    at: &str,
    markets: Vec<String>,
) -> Result<String, Vec<UserError>> {
    let state = mutation.state();
    let mut errors = Vec::new();
    let checked = check(state, input, at, None, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    let state = mutation.state_mut();
    let presence = state.new_gid("MarketWebPresence");
    let record = value::object(json!({
        "__typename": "MarketWebPresence",
        "id": presence,
        "domain": checked.domain,
        "subfolderSuffix": checked.suffix.map(|suffix| suffix.to_ascii_lowercase()),
        "defaultLocale": checked.default,
        "alternateLocales": checked.alternates.unwrap_or_default(),
        "markets": markets,
        "rootUrls": [],
    }));
    state.insert(record);
    refresh(state, &presence);
    domains_changed(state);
    state.emit("market_web_presences/create", &presence);
    Ok(presence)
}

/// Updates a presence. The errors, if any.
fn update_presence(
    mutation: &mut Mutation<'_>,
    presence: &str,
    input: &Map<String, Value>,
    at: &str,
) -> Vec<UserError> {
    let state = mutation.state();
    let mut errors = Vec::new();
    let checked = check(state, input, at, Some(presence), &mut errors);
    if !errors.is_empty() {
        return errors;
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(presence) {
        if let Some(domain) = checked.domain {
            record.insert("domain".into(), json!(domain));
            record.insert("subfolderSuffix".into(), Value::Null);
        }
        if let Some(suffix) = checked.suffix {
            record.insert("subfolderSuffix".into(), json!(suffix.to_ascii_lowercase()));
            record.insert("domain".into(), Value::Null);
        }
        if let Some(default) = checked.default {
            record.insert("defaultLocale".into(), json!(default));
        }
        if let Some(alternates) = checked.alternates {
            record.insert("alternateLocales".into(), json!(alternates));
        }
    }
    refresh(state, presence);
    domains_changed(state);
    state.emit("market_web_presences/update", presence);
    Vec::new()
}

/// Deletes a presence. The errors, if any.
fn delete_presence(mutation: &mut Mutation<'_>, presence: &str, field: &str) -> Vec<UserError> {
    let state = mutation.state();
    if primary_presence(state).as_deref() == Some(presence) {
        // lsf's wording.
        return vec![coded(
            &[field],
            "The web presence of the primary domain cannot be deleted",
            "SHOP_MUST_HAVE_PRIMARY_DOMAIN_WEB_PRESENCE",
        )];
    }
    let state = mutation.state_mut();
    state.delete(presence, "market_web_presences/delete");
    domains_changed(state);
    Vec::new()
}

fn not_found(field: &str) -> UserError {
    // lsf's wording.
    coded(
        &[field],
        "Web presence does not exist",
        "WEB_PRESENCE_NOT_FOUND",
    )
}

/// The presence of an argument.
fn presence_arg(state: &ShopState, ctx: &FieldContext<'_>, name: &str) -> Option<String> {
    let id = ctx.arg_str(name)?;
    state.is_of(id, "MarketWebPresence").then(|| id.to_string())
}

fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    match create_presence(mutation, &input, "input", Vec::new()) {
        Ok(presence) => Ok(payload(json!({"webPresence": presence}), &[])),
        Err(errors) => Ok(payload(json!({"webPresence": null}), &errors)),
    }
}

fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let Some(presence) = presence_arg(mutation.state(), ctx, "id") else {
        return Ok(payload(json!({"webPresence": null}), &[not_found("id")]));
    };
    // `WebPresenceUpdateInput` has no `domainId`: a presence keeps its domain.
    let input = ctx.arg_object_or_default("input");
    let errors = update_presence(mutation, &presence, &input, "input");
    let presence = errors.is_empty().then_some(presence);
    Ok(payload(json!({"webPresence": presence}), &errors))
}

fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let Some(presence) = presence_arg(mutation.state(), ctx, "id") else {
        return Ok(payload(json!({"deletedId": null}), &[not_found("id")]));
    };
    let errors = delete_presence(mutation, &presence, "id");
    let deleted = errors.is_empty().then_some(presence);
    Ok(payload(json!({"deletedId": deleted}), &errors))
}

/// `marketWebPresenceCreate` (deprecated): a presence of one market.
fn market_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let market = ctx
        .arg_str("marketId")
        .filter(|market| state.is_of(market, "Market"))
        .map(str::to_string);
    let Some(market) = market else {
        return Ok(payload(
            json!({"market": null}),
            &[market_not_found("marketId")],
        ));
    };
    let input = ctx.arg_object_or_default("webPresence");
    match create_presence(mutation, &input, "webPresence", vec![market.clone()]) {
        Ok(_) => {
            mutation.state_mut().emit("markets/update", &market);
            Ok(payload(json!({"market": market}), &[]))
        }
        Err(errors) => Ok(payload(json!({"market": null}), &errors)),
    }
}

/// The first market of a presence.
fn first_market(state: &ShopState, presence: &str) -> Option<String> {
    state
        .get(presence)
        .map(|record| strings(record.get("markets")))
        .unwrap_or_default()
        .into_iter()
        .min_by_key(|market| gid_number(market))
}

/// `marketWebPresenceUpdate` (deprecated).
fn market_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let Some(presence) = presence_arg(mutation.state(), ctx, "webPresenceId") else {
        return Ok(payload(
            json!({"market": null}),
            &[not_found("webPresenceId")],
        ));
    };
    let input = ctx.arg_object_or_default("webPresence");
    let errors = update_presence(mutation, &presence, &input, "webPresence");
    if !errors.is_empty() {
        return Ok(payload(json!({"market": null}), &errors));
    }
    let market = first_market(mutation.state(), &presence);
    Ok(payload(json!({"market": market}), &[]))
}

/// `marketWebPresenceDelete` (deprecated).
fn market_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let Some(presence) = presence_arg(mutation.state(), ctx, "webPresenceId") else {
        return Ok(payload(
            json!({"deletedId": null, "market": null}),
            &[not_found("webPresenceId")],
        ));
    };
    let market = first_market(mutation.state(), &presence);
    let errors = delete_presence(mutation, &presence, "webPresenceId");
    if !errors.is_empty() {
        return Ok(payload(json!({"deletedId": null, "market": null}), &errors));
    }
    if let Some(market) = &market {
        mutation.state_mut().emit("markets/update", market);
    }
    Ok(payload(
        json!({"deletedId": presence, "market": market}),
        &[],
    ))
}

/// Takes a locale out of every presence (a locale disabled), and refreshes their URLs.
pub fn remove_locale(state: &mut ShopState, locale: &str) {
    let presences: Vec<String> = state
        .keys("MarketWebPresence")
        .map(str::to_string)
        .collect();
    for presence in presences {
        let has = state.get(presence.as_str()).is_some_and(|record| {
            strings(record.get("alternateLocales"))
                .iter()
                .any(|known| known == locale)
        });
        if !has {
            continue;
        }
        state.pull_key(&presence, "alternateLocales", locale);
        refresh(state, &presence);
    }
    domains_changed(state);
}

/// Adds a locale to presences as an alternate (`shopLocaleEnable(marketWebPresenceIds:)`,
/// `shopLocaleUpdate`), or sets the presences that have it.
pub fn set_locale_presences(state: &mut ShopState, locale: &str, presences: &[String]) {
    let all: Vec<String> = state
        .keys("MarketWebPresence")
        .map(str::to_string)
        .collect();
    for presence in all {
        let wanted = presences.contains(&presence);
        let Some(record) = state.get(&presence) else {
            continue;
        };
        let is_default = record.get("defaultLocale").and_then(Value::as_str) == Some(locale);
        let has = strings(record.get("alternateLocales"))
            .iter()
            .any(|known| known == locale);
        if is_default || has == wanted {
            continue;
        }
        if let Some(Value::Array(alternates)) = state
            .get_mut(&presence)
            .and_then(|record| record.get_mut("alternateLocales"))
        {
            alternates.retain(|known| known.as_str() != Some(locale));
            if wanted {
                alternates.push(json!(locale));
            }
        }
        refresh(state, &presence);
    }
    domains_changed(state);
}

/// Whether a locale is the default locale of a presence.
pub fn is_default_locale(state: &ShopState, locale: &str) -> bool {
    state
        .all("MarketWebPresence")
        .any(|record| record.get("defaultLocale").and_then(Value::as_str) == Some(locale))
}
