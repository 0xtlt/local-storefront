//! Markets and what sells in them: markets and their regions, the backup region, web
//! presences, the shop's locales and the market localizations of resources, catalogs, price
//! lists (fixed prices, quantity rules and quantity price breaks, and the contextual pricing of
//! products and variants they give), publications, and the channels of a sales channel app with
//! their product and collection listings.
//!
//! The records (see "Markets, catalogs and channels" in `crates/admin/docs/design.md`):
//!
//! - **`Market`**: seeded by the content family (the markets the checkout sees); this family
//!   completes them ([`seed`]) and keeps them: `regions` and `conditions.regionsCondition
//!   .regions` hold the same inline `MarketRegionCountry`s, `currencySettings`,
//!   `priceInclusions`, `status` and `enabled`. A market's catalogs and web presences are read
//!   from the catalogs and the web presences that name it, which hold the link.
//! - **`MarketWebPresence`**: the primary domain's, seeded for every market, then those of
//!   `webPresenceCreate`. `ShopLocale.marketWebPresences`, `Domain.marketWebPresence` and
//!   `Market.webPresences` are read from them; a `Domain` keeps its `marketWebPresence` and
//!   `localization` too, which its webhook reports.
//! - **`MarketCatalog`**, **`CompanyLocationCatalog`**, **`AppCatalog`**: one id counter
//!   (`gid://shopify/Catalog/<n>` names any of them, as Shopify's examples do), their context
//!   (`markets`, `companyLocations`, `apps` and `channels`), `priceList`, `publication`.
//! - **`PriceList`**: its `parent` adjustment, its fixed prices (`lsfFixedPrices`), its quantity
//!   rules (`lsfQuantityRules`) and its **`QuantityPriceBreak`** records (`lsfPriceBreaks`).
//! - **`Publication`**: the Online Store's (the catalog family's), those of `publicationCreate`
//!   and those of the channels.
//! - **`Channel`**: made by `channelCreate`, with a publication and an `AppCatalog` of its own.
//! - **`ProductResourceFeedback`**: one per product and channel, under
//!   `feedback/<product GID>/<channel GID>` (it has no id).
//!
//! The backup region is a setting of the shop (`backupRegion`, a country code). Each module's
//! header says what shopify.dev documents of its mutations and where lsf differs.

mod catalogs;
mod channels;
mod locales;
mod localizations;
mod market;
mod price_lists;
mod publications;
#[cfg(test)]
mod tests;
mod web_presences;

pub use channels::{LISTING, after_mutation, listed, removing, scheduled};
pub use price_lists::{contextual_price, rate_bps, removing_variants};

use lsf_core::store::reference;
use lsf_core::util::stable_id;
use serde_json::{Map, Value, json};

use crate::domains::catalog;
use crate::graphql::FieldContext;
use crate::resolve::{Registry, UserError};
use crate::state::seed::Seed;
use crate::state::{Record, ShopState, gid, parse_gid};

/// The setting that holds the backup region: the code of its country.
pub const BACKUP_REGION: &str = "backupRegion";

/// The record types of catalogs: they share one id counter, as `Catalog/<n>` names any.
pub const CATALOG_TYPES: [&str; 3] = ["MarketCatalog", "CompanyLocationCatalog", "AppCatalog"];

/// The `MarketRegionCountry` of a country, as the seed writes it: `stable_id("market_region",
/// code)`.
pub fn region_gid(code: &str) -> String {
    gid("MarketRegionCountry", stable_id("market_region", code))
}

/// The web presence of a domain, or of a subfolder of the primary domain.
pub fn web_presence_gid(key: &str) -> String {
    gid("MarketWebPresence", stable_id("market_web_presence", key))
}

/// The `AppCatalog` of the Online Store's publication.
pub fn online_store_catalog_gid() -> String {
    gid("AppCatalog", stable_id("app_catalog", "online-store"))
}

pub fn register(registry: &mut Registry) {
    market::register(registry);
    web_presences::register(registry);
    locales::register(registry);
    localizations::register(registry);
    catalogs::register(registry);
    price_lists::register(registry);
    publications::register(registry);
    channels::register(registry);
}

// --- The seed ---

/// Completes what the content family seeds of markets, locales and domains, and seeds the web
/// presence of the primary domain, the Online Store's catalog and the backup region. Runs after
/// the content and app families' seeds.
pub fn seed(seed: &mut Seed) {
    let state = &mut seed.state;
    let markets: Vec<String> = state.keys("Market").map(str::to_string).collect();
    let taxes_included = seed.store.shop.taxes_included;
    for market in &markets {
        if let Some(record) = state.get_mut(market) {
            complete_market(record, taxes_included);
        }
    }
    // The web presence of the primary domain: every market's, in every locale of the shop.
    let primary_domain = state
        .shop()
        .and_then(|shop| shop.get("primaryDomain"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let host = primary_domain
        .as_deref()
        .and_then(|domain| state.get(domain))
        .and_then(|domain| domain.get("host"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let locales = shop_locales(state);
    let (default, alternates) = match locales.split_first() {
        Some((first, rest)) => (first.clone(), rest.to_vec()),
        None => ("en".to_string(), Vec::new()),
    };
    let presence = web_presence_gid(&host);
    state.insert(crate::state::value::object(json!({
        "__typename": "MarketWebPresence",
        "id": presence,
        "domain": primary_domain,
        "subfolderSuffix": null,
        "defaultLocale": default,
        "alternateLocales": alternates,
        "markets": markets,
        "rootUrls": [],
    })));
    web_presences::refresh(state, &presence);
    web_presences::refresh_domains(state);
    // The Online Store's catalog.
    let online_store = catalog::online_store_publication_gid();
    let app_catalog = online_store_catalog_gid();
    state.insert(crate::state::value::object(json!({
        "__typename": "AppCatalog",
        "id": app_catalog,
        "title": "Online Store",
        "status": "ACTIVE",
        "priceList": null,
        "publication": online_store,
        "operations": [],
        "apps": [],
        "channels": [],
    })));
    if let Some(record) = state.get_mut(&online_store) {
        complete_publication(record, Some(&app_catalog));
    }
    // The backup region: the shop's country, else the first market's.
    let country = seed.store.shop.address.country_code.to_ascii_uppercase();
    let backup = if regions_of_all(state).contains(&country) {
        country
    } else {
        regions_of_all(state).into_iter().next().unwrap_or(country)
    };
    state.set_setting(BACKUP_REGION, json!(backup));
}

/// The fields of a market the content family's seed leaves out, as a market created through
/// the Admin API has them.
fn complete_market(record: &mut Record, taxes_included: bool) {
    if let Some(Value::Array(regions)) = record.get_mut("regions") {
        for region in regions.iter_mut() {
            complete_currency(region.get_mut("currency"));
        }
    }
    if let Some(Value::Array(regions)) = record
        .get_mut("conditions")
        .and_then(|conditions| conditions.get_mut("regionsCondition"))
        .and_then(|condition| condition.get_mut("regions"))
    {
        for region in regions.iter_mut() {
            complete_currency(region.get_mut("currency"));
        }
    }
    if let Some(Value::Object(conditions)) = record.get_mut("conditions") {
        if let Some(Value::Object(regions)) = conditions.get_mut("regionsCondition") {
            regions
                .entry("applicationLevel")
                .or_insert(json!("SPECIFIED"));
        }
        for condition in [
            "companyLocationsCondition",
            "locationsCondition",
            "channelsCondition",
        ] {
            conditions.entry(condition).or_insert(Value::Null);
        }
    }
    if let Some(Value::Object(settings)) = record.get_mut("currencySettings") {
        settings.entry("roundingEnabled").or_insert(json!(false));
        complete_currency(settings.get_mut("baseCurrency"));
    }
    record
        .entry("priceInclusions")
        .or_insert_with(|| price_inclusions(taxes_included, None));
    record
        .entry("delivery")
        .or_insert(json!({"shipping": null}));
    for field in ["returnPolicyProfile", "priceList"] {
        record.entry(field).or_insert(Value::Null);
    }
    for field in ["childMarkets", "parentMarkets", "discounts"] {
        record.entry(field).or_insert(json!([]));
    }
}

/// A `CurrencySetting` with the fields the seed leaves out: no manual rate.
fn complete_currency(setting: Option<&mut Value>) {
    if let Some(Value::Object(setting)) = setting {
        setting.entry("manualRate").or_insert(Value::Null);
        setting.entry("rateUpdatedAt").or_insert(Value::Null);
    }
}

/// A market's `priceInclusions`: taxes in the price when the shop's prices include them (the
/// store data's `taxes_included`), duties added at checkout, no adaptive pricing; or what the
/// input gives.
fn price_inclusions(taxes_included: bool, input: Option<&Map<String, Value>>) -> Value {
    let given = |name: &str| input.and_then(|input| input.get(name)).cloned();
    json!({
        "adaptivePricingEnabled": given("adaptivePricingEnabled")
            .filter(|value| !value.is_null())
            .unwrap_or(json!(false)),
        "inclusiveDutiesPricingStrategy": given("dutiesPricingStrategy")
            .filter(|value| !value.is_null())
            .unwrap_or(json!("ADD_DUTIES_AT_CHECKOUT")),
        "inclusiveTaxPricingStrategy": given("taxPricingStrategy")
            .filter(|value| !value.is_null())
            .unwrap_or(json!(if taxes_included {
                "INCLUDES_TAXES_IN_PRICE"
            } else {
                "ADD_TAXES_AT_CHECKOUT"
            })),
    })
}

/// The fields of a publication the catalog family's seed leaves out.
fn complete_publication(record: &mut Record, catalog: Option<&str>) {
    record.entry("autoPublish").or_insert(json!(false));
    // lsf publishes on the Online Store at once, whatever the `publishDate`.
    record
        .entry("supportsFuturePublishing")
        .or_insert(json!(false));
    record.entry("catalog").or_insert(json!(catalog));
    record.entry("operation").or_insert(Value::Null);
    record.entry("channels").or_insert(json!([]));
    record.entry("collections").or_insert(json!([]));
    record.entry("products").or_insert(json!([]));
}

// --- What the modules share ---

/// The shop's locales, the primary first: the keys of the `ShopLocale` records.
pub fn shop_locales(state: &ShopState) -> Vec<String> {
    let mut locales: Vec<(bool, String)> = state
        .all("ShopLocale")
        .filter_map(|locale| {
            Some((
                locale.get("primary").and_then(Value::as_bool) != Some(true),
                locale.get("locale")?.as_str()?.to_string(),
            ))
        })
        .collect();
    locales.sort_by_key(|(secondary, _)| *secondary);
    locales.into_iter().map(|(_, locale)| locale).collect()
}

/// The primary locale of the shop.
pub fn primary_locale(state: &ShopState) -> String {
    shop_locales(state)
        .into_iter()
        .next()
        .unwrap_or_else(|| "en".to_string())
}

/// The country codes of a market's regions, in order.
pub fn regions_of(record: &Record) -> Vec<String> {
    record
        .get("regions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|region| region.get("code")?.as_str())
        .map(str::to_string)
        .collect()
}

/// The countries of every market, in the order of the markets.
fn regions_of_all(state: &ShopState) -> Vec<String> {
    state.all("Market").flat_map(regions_of).collect()
}

/// Whether a market is active (`status: ACTIVE`; the deprecated `enabled` follows it).
pub fn is_active(record: &Record) -> bool {
    record.get("status").and_then(Value::as_str) != Some("DRAFT")
}

/// The shop's currency.
pub fn shop_currency(state: &ShopState) -> String {
    state
        .shop()
        .and_then(|shop| shop.get("currencyCode"))
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string()
}

/// The market a buyer in a country is in: the first active market of the region whose
/// regions have the country (a company location or a location market does not match a
/// country alone).
pub fn market_of_country(state: &ShopState, country: &str) -> Option<String> {
    state
        .keys("Market")
        .find(|market| {
            state.get(market).is_some_and(|record| {
                is_active(record)
                    && record.get("type").and_then(Value::as_str) == Some("REGION")
                    && regions_of(record)
                        .iter()
                        .any(|code| code.eq_ignore_ascii_case(country))
            })
        })
        .map(str::to_string)
}

/// The currency a market sells in: its base currency.
pub fn market_currency(state: &ShopState, market: &str) -> String {
    state
        .get(market)
        .and_then(|record| record.pointer_str("/currencySettings/baseCurrency/currencyCode"))
        .map_or_else(|| shop_currency(state), str::to_string)
}

/// A `CurrencySetting` of a currency, as the seed writes them.
pub fn currency_setting(code: &str) -> Value {
    json!({
        "currencyCode": code,
        "currencyName": reference::currency_name(code),
        "enabled": true,
        "manualRate": null,
        "rateUpdatedAt": null,
    })
}

/// The inline `MarketRegionCountry` of a country: as some market has it already (the store
/// data's name and currency), else with Shopify's name of the country and its local currency.
pub fn region(state: &ShopState, code: &str) -> Value {
    let code = code.to_ascii_uppercase();
    if let Some(known) = state
        .all("Market")
        .flat_map(|market| {
            market
                .get("regions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .find(|region| region.get("code").and_then(Value::as_str) == Some(code.as_str()))
    {
        return known;
    }
    let currency =
        reference::country_currency(&code).map_or_else(|| shop_currency(state), str::to_string);
    json!({
        "__typename": "MarketRegionCountry",
        "id": region_gid(&code),
        "code": code,
        "name": reference::country_name(&code).unwrap_or(&code),
        "currency": currency_setting(&currency),
    })
}

/// A string at a JSON pointer of a record.
trait Pointer {
    fn pointer_str(&self, pointer: &str) -> Option<&str>;
}

impl Pointer for Record {
    fn pointer_str(&self, pointer: &str) -> Option<&str> {
        let (first, rest) = pointer.trim_start_matches('/').split_once('/')?;
        self.get(first)?.pointer(&format!("/{rest}"))?.as_str()
    }
}

/// An argument of the field that is an input object (`{}` when it is not given).
fn input(ctx: &FieldContext<'_>, name: &str) -> Map<String, Value> {
    ctx.arg(name)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// A string field of an input.
fn text<'i>(input: &'i Map<String, Value>, field: &str) -> Option<&'i str> {
    input.get(field).and_then(Value::as_str)
}

/// The strings of a list field of an input (the GIDs of `marketIds`).
fn strings(value: Option<&Value>) -> Vec<String> {
    catalog::gids(value)
}

/// The objects of a list (an argument or a field of an input).
fn objects(value: Option<&Value>) -> Vec<Map<String, Value>> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .cloned()
        .collect()
}

/// Whether `key` is a record of `type_name`.
fn is(state: &ShopState, key: &str, type_name: &str) -> bool {
    state.type_of(key) == Some(type_name)
}

/// The record a catalog GID names: `Catalog/<n>` is the catalog of that number, whatever its
/// type, as Shopify's examples name them.
pub fn catalog_key(state: &ShopState, id: &str) -> Option<String> {
    if state
        .type_of(id)
        .is_some_and(|type_name| CATALOG_TYPES.contains(&type_name))
    {
        return Some(id.to_string());
    }
    let (type_name, number) = parse_gid(id)?;
    (type_name == "Catalog")
        .then(|| {
            CATALOG_TYPES
                .iter()
                .map(|type_name| gid(type_name, number))
                .find(|key| state.contains(key))
        })
        .flatten()
}

/// A user error with a code.
fn coded(field: &[&str], message: impl Into<String>, code: &str) -> UserError {
    UserError::new(field, message).code(code)
}

/// The numeric id of a GID, for sorting a list as Shopify answers it (by id).
fn number(gid: &str) -> u64 {
    parse_gid(gid).map_or(0, |(_, id)| id)
}
