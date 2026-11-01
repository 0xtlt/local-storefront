//! Markets: `marketCreate`, `marketUpdate`, `marketDelete`, the deprecated
//! `marketCurrencySettingsUpdate`, `marketRegionsCreate`, `marketRegionDelete` and
//! `marketRegionsDelete`, `backupRegionUpdate`; the `markets(type:)`, `marketByGeography`,
//! `primaryMarket`, `backupRegion` and `marketsResolvedValues` queries, and the fields of a
//! market lsf computes (`catalogs`, `assignedCustomization`, `priceList`).
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A market matches buyers by its `conditions`: regions (countries), company locations,
//!   locations (retail), channels. Its `type` follows them (`REGION`, `COMPANY_LOCATION`,
//!   `LOCATION`, `CHANNEL`, `NONE` without any). A draft market "never matches any buyers"
//!   (`marketUpdate`'s example). `enabled` is the deprecated mirror of `status`.
//! - The handle is made from the name when it is not given, and written as a handle
//!   (`"Company-Location"` → `company-location`, `"NA"` → `na`, as the examples answer).
//! - A country is in one active region market at most: `makeDuplicateUniqueMarketsDraft` puts
//!   the other markets of its countries in `DRAFT` (`DUPLICATE_UNIQUE_MARKET` otherwise). The
//!   store data keys a country to one market too (`duplicate_market_country`).
//! - `catalogs` (`catalogsToAdd`, `catalogsToDelete`) and `webPresences` link the market to
//!   market catalogs and web presences; the market's catalogs and web presences are read from
//!   them.
//! - The primary market (the shop's country's: `primary`, deprecated) cannot be deleted or put
//!   in draft (`CANNOT_DELETE_PRIMARY_MARKET`, `CANNOT_DISABLE_PRIMARY_MARKET`).
//! - Events: `markets/create`, `markets/update`, `markets/delete`; `backupRegionUpdate`
//!   `markets_backup_region/update`.
//!
//! The messages are lsf's where shopify.dev publishes none (each says so): the codes are the
//! schema's. Not simulated, and said in `extensions.lsf.notes`: `delivery`, the return policy
//! profile, subdivisions of a country, channel conditions (kept, matched by nothing).

use lsf_core::util::handleize;
use serde_json::{Map, Value, json};

use super::{
    BACKUP_REGION, Pointer, is_active, market_currency, market_not_found, market_of_country,
    objects, price_inclusions, records_input, region, region_gid, regions_of, strings, text,
    web_presences,
};
use crate::domains::content::market_gid;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{
    Mutation, Obj, Registry, ShopResolver, UserError, coded, input_path, payload,
};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("marketCreate", create)
        .mutation("marketUpdate", update)
        .mutation("marketDelete", delete)
        .mutation("marketCurrencySettingsUpdate", currency_settings_update)
        .mutation("marketRegionsCreate", regions_create)
        .mutation("marketRegionDelete", region_delete)
        .mutation("marketRegionsDelete", regions_delete)
        .mutation("backupRegionUpdate", backup_region_update)
        .query("markets", markets)
        .query("marketByGeography", market_by_geography)
        .query("primaryMarket", primary_market)
        .query("backupRegion", backup_region)
        .query("marketsResolvedValues", resolved_values)
        .field("Market", "catalogs", market_catalogs)
        .field("Market", "catalogsCount", market_catalogs_count)
        .field("Market", "assignedCustomization", |_r, _ctx, _object| {
            Ok(Output::value(false))
        })
        .field("Market", "priceList", market_price_list)
        .field("Customer", "market", customer_market)
        .field("MarketsResolvedValues", "catalogs", resolved_catalogs)
        .field(
            "MarketsResolvedValues",
            "webPresences",
            resolved_web_presences,
        );
}

// --- Queries ---

/// `markets(type:)`: the markets of a type (all of them without one), the `query` and the sort
/// key (`NAME` by default) applied by the connection.
fn markets(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let wanted = ctx.arg_str("type");
    let items: Vec<Value> = r
        .state()
        .keys("Market")
        .filter(|market| {
            wanted.is_none_or(|wanted| {
                r.state().get(market).and_then(|record| record.get("type")) == Some(&json!(wanted))
            })
        })
        .map(|market| json!(market))
        .collect();
    r.connection(ctx, items)
}

/// `marketByGeography(countryCode:)`: the active region market of the country.
fn market_by_geography(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let country = ctx.arg_str("countryCode").unwrap_or_default();
    Ok(Output::object_or_null(
        market_of_country(r.state(), country).map(|market| Obj::record(&market)),
    ))
}

/// The primary market: the shop's country's (`primary`), else the first.
fn primary_of(state: &ShopState) -> Option<String> {
    state
        .keys("Market")
        .find(|market| {
            state
                .get(market)
                .and_then(|record| record.get("primary"))
                .and_then(Value::as_bool)
                == Some(true)
        })
        .or_else(|| state.keys("Market").next())
        .map(str::to_string)
}

fn primary_market(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(Output::object_or_null(
        primary_of(r.state()).map(|market| Obj::record(&market)),
    ))
}

/// The backup region of the shop: the `MarketRegionCountry` of its country.
fn backup_region_value(state: &ShopState) -> Value {
    let code = state
        .setting(BACKUP_REGION)
        .and_then(Value::as_str)
        .unwrap_or("US");
    region(state, code)
}

fn backup_region(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Value::Object(region) = backup_region_value(r.state()) else {
        return Ok(Output::Null);
    };
    Ok(Output::Object(Obj::inline("MarketRegionCountry", region)))
}

/// `marketsResolvedValues(buyerSignal: {countryCode})`: what a buyer of the country gets: the
/// market's currency, its price inclusivity, its catalogs and web presences (the backup
/// region's market's for a country in none).
fn resolved_values(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let country = ctx
        .arg("buyerSignal")
        .and_then(|signal| signal.get("countryCode"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let market = market_of_country(state, country).or_else(|| {
        let backup = state.setting(BACKUP_REGION)?.as_str()?;
        market_of_country(state, backup)
    });
    let record = market.as_deref().and_then(|market| state.get(market));
    let strategy = |pointer: &str| {
        record
            .and_then(|record| record.pointer_str(pointer))
            .map(str::to_string)
    };
    let taxes = strategy("/priceInclusions/inclusiveTaxPricingStrategy")
        .is_some_and(|strategy| strategy.starts_with("INCLUDES_TAXES"));
    let duties = strategy("/priceInclusions/inclusiveDutiesPricingStrategy").as_deref()
        == Some("INCLUDE_DUTIES_IN_PRICE");
    let currency = market
        .as_deref()
        .map_or_else(|| state.currency(), |market| market_currency(state, market));
    let value = value::object(json!({
        "market": market,
        "currencyCode": currency,
        "priceInclusivity": {"dutiesIncluded": duties, "taxesIncluded": taxes},
    }));
    Ok(Output::Object(Obj::built("MarketsResolvedValues", value)))
}

fn resolved_market(r: &ShopResolver<'_>, object: &Obj) -> Option<String> {
    r.map_of(object)?
        .get("market")?
        .as_str()
        .map(str::to_string)
}

fn resolved_catalogs(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = resolved_market(r, object)
        .map(|market| catalogs_of(r.state(), &market, true))
        .unwrap_or_default();
    r.connection(ctx, items)
}

fn resolved_web_presences(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = resolved_market(r, object)
        .map(|market| web_presences::of_market(r.state(), &market))
        .unwrap_or_default();
    r.connection(ctx, items)
}

/// The market catalogs of a market (only the active ones, for what a buyer gets).
pub fn catalogs_of(state: &ShopState, market: &str, active_only: bool) -> Vec<Value> {
    state
        .keys("MarketCatalog")
        .filter(|catalog| {
            state.get(catalog).is_some_and(|record| {
                strings(record.get("markets"))
                    .iter()
                    .any(|known| known == market)
                    && (!active_only
                        || record.get("status").and_then(Value::as_str) == Some("ACTIVE"))
            })
        })
        .map(|catalog| json!(catalog))
        .collect()
}

fn market_catalogs(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let market = object.key().unwrap_or_default();
    r.connection(ctx, catalogs_of(r.state(), market, false))
}

fn market_catalogs_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let market = object.key().unwrap_or_default();
    Ok(r.count(ctx, catalogs_of(r.state(), market, false).len()))
}

/// `Market.priceList` (deprecated): the price list of its first catalog that has one.
fn market_price_list(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let market = object.key().unwrap_or_default();
    let list = catalogs_of(state, market, false)
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|catalog| state.get(catalog)?.get("priceList")?.as_str())
        .find(|list| state.contains(list))
        .map(Obj::record);
    Ok(Output::object_or_null(list))
}

/// `Customer.market`: the market of the country of the customer's default address.
fn customer_market(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let country = r
        .map_of(object)
        .and_then(|customer| customer.get("defaultAddress"))
        .and_then(|address| {
            address
                .get("countryCodeV2")
                .or_else(|| address.get("countryCode"))
        })
        .and_then(Value::as_str);
    Ok(Output::object_or_null(
        country
            .and_then(|country| market_of_country(state, country))
            .map(|market| Obj::record(&market)),
    ))
}

// --- What the mutations share ---

/// The market of an `id` argument.
fn market_arg(state: &ShopState, ctx: &FieldContext<'_>, name: &str) -> Option<String> {
    let id = ctx.arg_str(name)?;
    state.is_of(id, "Market").then(|| id.to_string())
}

/// The regions of a `MarketConditionsRegionsInput` (`regions` of country codes, or
/// `regionIds`), as inline `MarketRegionCountry`s, or the errors at `at`.
fn regions_input(
    state: &ShopState,
    condition: &Map<String, Value>,
    at: &[&str],
    errors: &mut Vec<UserError>,
    notes: &mut Vec<String>,
) -> Vec<Value> {
    let mut regions = Vec::new();
    for item in objects(condition.get("regions")) {
        let Some(code) = text(&item, "countryCode") else {
            continue;
        };
        if item
            .get("subdivision")
            .is_some_and(|value| !value.is_null())
        {
            notes.push(format!(
                "lsf does not simulate the subdivisions of a market's region: {code} is the whole country"
            ));
        }
        regions.push(region(state, code));
    }
    for (index, id) in strings(condition.get("regionIds")).iter().enumerate() {
        match known_region(state, id) {
            Some(region) => regions.push(region),
            None => {
                let index = index.to_string();
                let field = input_path(at, &["regionIds", &index]);
                // lsf's wording.
                errors.push(coded(&field, "Region does not exist", "REGION_NOT_FOUND"));
            }
        }
    }
    let mut seen = Vec::new();
    regions.retain(|region| {
        let code = region.get("code").cloned();
        if seen.contains(&code) {
            false
        } else {
            seen.push(code);
            true
        }
    });
    regions
}

/// The region of a `MarketRegionCountry` GID: one some market has, or a country's whose GID it
/// is (`stable_id("market_region", code)`).
fn known_region(state: &ShopState, id: &str) -> Option<Value> {
    state
        .all("Market")
        .flat_map(|market| {
            market
                .get("regions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .find(|region| region.get("id").and_then(Value::as_str) == Some(id))
        .cloned()
        .or_else(|| {
            lsf_core::store::reference::country_names()
                .iter()
                .filter_map(|name| lsf_core::store::reference::country_code_from_name(name))
                .find(|code| region_gid(code) == id)
                .map(|code| region(state, code))
        })
}

/// The `type` of a market of its conditions.
fn type_of_conditions(conditions: &Map<String, Value>) -> &'static str {
    let has = |name: &str| conditions.get(name).is_some_and(|value| !value.is_null());
    if has("companyLocationsCondition") {
        "COMPANY_LOCATION"
    } else if has("locationsCondition") {
        "LOCATION"
    } else if has("channelsCondition") {
        "CHANNEL"
    } else if has("regionsCondition") {
        "REGION"
    } else {
        "NONE"
    }
}

/// The `conditionTypes` of conditions, in the order of the enum.
fn condition_types(conditions: &Map<String, Value>) -> Vec<&'static str> {
    [
        ("channelsCondition", "CHANNEL"),
        ("companyLocationsCondition", "COMPANY_LOCATION"),
        ("locationsCondition", "LOCATION"),
        ("regionsCondition", "REGION"),
    ]
    .into_iter()
    .filter(|(name, _)| conditions.get(*name).is_some_and(|value| !value.is_null()))
    .map(|(_, kind)| kind)
    .collect()
}

/// A condition of records (`companyLocationsCondition`, `locationsCondition`,
/// `channelsCondition`) from its input: `applicationLevel` `ALL` (no list: every one, which
/// Shopify answers as an empty connection), or the records given.
fn records_condition(
    state: &ShopState,
    input: &Map<String, Value>,
    list: &str,
    ids_field: &str,
    type_name: &str,
    at: &[&str],
    errors: &mut Vec<UserError>,
) -> Value {
    if text(input, "applicationLevel") == Some("ALL") {
        return json!({"applicationLevel": "ALL", list: []});
    }
    let field = input_path(at, &[ids_field]);
    // lsf's wording.
    let found = records_input(
        state,
        &strings(input.get(ids_field)),
        type_name,
        &field,
        "Condition does not exist",
        "CONDITIONS_NOT_FOUND",
        errors,
    );
    json!({"applicationLevel": "SPECIFIED", list: found})
}

/// The conditions of a market, from a `MarketConditionsInput`.
fn conditions_input(
    state: &ShopState,
    conditions: &Map<String, Value>,
    at: &[&str],
    errors: &mut Vec<UserError>,
    notes: &mut Vec<String>,
) -> (Map<String, Value>, Vec<Value>) {
    let mut built = Map::new();
    let mut regions = Vec::new();
    if let Some(Value::Object(condition)) = conditions.get("regionsCondition") {
        let field = [at, &["regionsCondition"]].concat();
        regions = regions_input(state, condition, &field, errors, notes);
        let level = if text(condition, "applicationLevel") == Some("ALL") {
            "ALL"
        } else {
            "SPECIFIED"
        };
        built.insert(
            "regionsCondition".into(),
            json!({"applicationLevel": level, "regions": regions}),
        );
    }
    for (name, list, ids, type_name) in [
        (
            "companyLocationsCondition",
            "companyLocations",
            "companyLocationIds",
            "CompanyLocation",
        ),
        ("locationsCondition", "locations", "locationIds", "Location"),
        ("channelsCondition", "channels", "channelIds", "Channel"),
    ] {
        if let Some(Value::Object(condition)) = conditions.get(name) {
            let field = [at, &[name]].concat();
            let value = records_condition(state, condition, list, ids, type_name, &field, errors);
            built.insert(name.into(), value);
        }
    }
    (built, regions)
}

/// Writes conditions into a market: `conditions` with its `conditionTypes`, `regions`, `type`.
fn set_conditions(record: &mut Record, mut conditions: Map<String, Value>, regions: Vec<Value>) {
    let types = condition_types(&conditions);
    let kind = type_of_conditions(&conditions);
    for name in [
        "regionsCondition",
        "companyLocationsCondition",
        "locationsCondition",
        "channelsCondition",
    ] {
        conditions.entry(name).or_insert(Value::Null);
    }
    conditions.insert("conditionTypes".into(), json!(types));
    record.insert("conditions".into(), Value::Object(conditions));
    record.insert("regions".into(), json!(regions));
    record.insert("type".into(), json!(kind));
}

/// The countries of a market that another active region market has: `(country, market)`.
fn duplicates(state: &ShopState, market: Option<&str>, countries: &[String]) -> Vec<String> {
    state
        .keys("Market")
        .filter(|other| Some(*other) != market)
        .filter(|other| {
            state.get(other).is_some_and(|record| {
                is_active(record)
                    && record.get("type").and_then(Value::as_str) == Some("REGION")
                    && regions_of(record)
                        .iter()
                        .any(|code| countries.contains(code))
            })
        })
        .map(str::to_string)
        .collect()
}

/// A unique handle for a market: `handle`, else `handle-1`, `handle-2`, …
fn unique_handle(state: &ShopState, handle: &str, market: Option<&str>) -> String {
    crate::state::free_handle(handle, |candidate| {
        state.handle_taken("Market", candidate, None, market)
    })
}

/// The currency settings of a market from a `MarketCurrencySettingsUpdateInput`, over what it
/// has (`existing`), its regions giving the base currency when none is given: the currency of
/// its countries when they share one, else the shop's.
fn currency_settings(
    state: &ShopState,
    input: Option<&Map<String, Value>>,
    existing: Option<&Value>,
    regions: &[Value],
) -> Value {
    let shop = state.currency();
    let local: Vec<String> = regions
        .iter()
        .filter_map(|region| region.pointer("/currency/currencyCode")?.as_str())
        .map(str::to_string)
        .collect();
    let derived_base = local
        .iter()
        .find(|currency| **currency != shop)
        .cloned()
        .unwrap_or_else(|| shop.clone());
    let existing_base = existing
        .and_then(|settings| settings.pointer("/baseCurrency/currencyCode"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let given = |name: &str| {
        input
            .and_then(|input| input.get(name))
            .filter(|v| !v.is_null())
    };
    let base = given("baseCurrency")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(existing_base)
        .unwrap_or(derived_base);
    let local_currencies = given("localCurrencies")
        .and_then(Value::as_bool)
        .or_else(|| existing.and_then(|settings| settings.get("localCurrencies")?.as_bool()))
        .unwrap_or_else(|| local.iter().any(|currency| *currency != shop));
    let rounding = given("roundingEnabled")
        .and_then(Value::as_bool)
        .or_else(|| existing.and_then(|settings| settings.get("roundingEnabled")?.as_bool()))
        .unwrap_or(false);
    let mut setting = super::currency_setting(&base);
    if let Some(rate) = given("baseCurrencyManualRate") {
        setting["manualRate"] = rate.clone();
        setting["rateUpdatedAt"] = json!(state.timestamp());
    } else if let Some(previous) = existing.and_then(|settings| settings.get("baseCurrency"))
        && previous.get("currencyCode").and_then(Value::as_str) == Some(base.as_str())
    {
        setting["manualRate"] = previous.get("manualRate").cloned().unwrap_or(Value::Null);
        setting["rateUpdatedAt"] = previous
            .get("rateUpdatedAt")
            .cloned()
            .unwrap_or(Value::Null);
    }
    json!({
        "baseCurrency": setting,
        "localCurrencies": local_currencies,
        "roundingEnabled": rounding,
    })
}

/// The `status` and `enabled` of an input: `INVALID_STATUS_AND_ENABLED_COMBINATION` when they
/// disagree.
fn status_input(input: &Map<String, Value>, errors: &mut Vec<UserError>) -> Option<&'static str> {
    let status = text(input, "status");
    let enabled = input.get("enabled").and_then(Value::as_bool);
    match (status, enabled) {
        (Some("ACTIVE"), Some(false)) | (Some("DRAFT"), Some(true)) => {
            // lsf's wording.
            errors.push(coded(
                &["input", "status"],
                "Status and enabled do not agree",
                "INVALID_STATUS_AND_ENABLED_COMBINATION",
            ));
            None
        }
        (Some("DRAFT"), _) | (None, Some(false)) => Some("DRAFT"),
        (Some(_), _) | (None, Some(true)) => Some("ACTIVE"),
        (None, None) => None,
    }
}

/// Links a market to market catalogs, or unlinks it.
fn link_catalogs(state: &mut ShopState, market: &str, catalogs: &[String], linked: bool) {
    for catalog in catalogs {
        if let Some(Value::Array(markets)) = state
            .get_mut(catalog)
            .and_then(|record| record.get_mut("markets"))
        {
            markets.retain(|known| known.as_str() != Some(market));
            if linked {
                markets.push(json!(market));
            }
        }
    }
}

/// The market catalogs of a list of an input, or the errors at `at`.
fn catalogs_input(
    state: &ShopState,
    ids: &[String],
    at: &[&str],
    errors: &mut Vec<UserError>,
) -> Vec<String> {
    let mut found = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let index = index.to_string();
        let field = input_path(at, &[&index]);
        match super::catalog_key(state, id) {
            Some(key) if state.is_of(&key, "MarketCatalog") => found.push(key),
            // lsf's wording.
            Some(_) => errors.push(coded(
                &field,
                "Only market catalogs can be added to a market",
                "CATALOG_TYPE_NOT_SUPPORTED",
            )),
            None => errors.push(coded(&field, "Catalog does not exist", "INVALID")),
        }
    }
    found
}

/// The discounts of a list of an input, or the errors at `at`.
fn discounts_input(
    state: &ShopState,
    ids: &[String],
    at: &[&str],
    errors: &mut Vec<UserError>,
) -> Vec<String> {
    let mut found = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let alias = crate::state::parse_gid(id).map(|(_, number)| {
            [
                crate::state::gid("DiscountAutomaticNode", number),
                crate::state::gid("DiscountCodeNode", number),
            ]
        });
        let key = std::iter::once(id.clone())
            .chain(alias.into_iter().flatten())
            .find(|key| {
                matches!(
                    state.type_of(key),
                    Some("DiscountAutomaticNode" | "DiscountCodeNode")
                )
            });
        match key {
            Some(key) => found.push(key),
            None => {
                let index = index.to_string();
                let field = input_path(at, &[&index]);
                // lsf's wording.
                errors.push(coded(&field, "Discount does not exist", "INVALID"));
            }
        }
    }
    found
}

/// Puts the other markets of a market's countries in draft: `makeDuplicateUniqueMarketsDraft`.
fn draft_duplicates(state: &mut ShopState, others: &[String]) {
    for other in others {
        if let Some(record) = state.get_mut(other) {
            record.insert("status".into(), json!("DRAFT"));
            record.insert("enabled".into(), json!(false));
        }
        state.emit("markets/update", other);
    }
}

/// The notes of the inputs lsf keeps without simulating them.
fn note_ignored(mutation: &mut Mutation<'_>, name: &str, input: &Map<String, Value>) {
    for field in ["delivery", "returnPolicyProfileId"] {
        if input.get(field).is_some_and(|value| !value.is_null()) {
            mutation.note(format!(
                "{name}: lsf does not simulate `{field}`: it was ignored"
            ));
        }
    }
}

// --- Mutations ---

/// `marketCreate`.
fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let state = mutation.state();
    let mut errors = Vec::new();
    let mut notes = Vec::new();
    let name = text(&input, "name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        // Rails' wording, as the other `BLANK` errors of the Admin API.
        errors.push(coded(&["input", "name"], "Name can't be blank", "BLANK"));
    } else if state.all("Market").any(|market| {
        market
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|known| known.eq_ignore_ascii_case(&name))
    }) {
        errors.push(coded(
            &["input", "name"],
            "Name has already been taken",
            "TAKEN",
        ));
    }
    let handle = match text(&input, "handle").map(handleize) {
        Some(handle) if handle.is_empty() => {
            errors.push(coded(
                &["input", "handle"],
                "Handle can't be blank",
                "BLANK",
            ));
            String::new()
        }
        Some(handle) => {
            if unique_handle(state, &handle, None) != handle {
                errors.push(coded(
                    &["input", "handle"],
                    "Handle has already been taken",
                    "TAKEN",
                ));
            }
            handle
        }
        None => unique_handle(state, &handleize(&name), None),
    };
    let status = status_input(&input, &mut errors).unwrap_or("ACTIVE");
    let conditions_given = input
        .get("conditions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let (mut conditions, mut regions) = conditions_input(
        state,
        &conditions_given,
        &["input", "conditions"],
        &mut errors,
        &mut notes,
    );
    // The deprecated `regions` of country codes.
    let legacy = objects(input.get("regions"));
    if !legacy.is_empty() {
        let mut condition = Map::new();
        condition.insert("regions".into(), json!(legacy));
        let more = regions_input(state, &condition, &["input"], &mut errors, &mut notes);
        regions.extend(more);
        conditions.insert(
            "regionsCondition".into(),
            json!({"applicationLevel": "SPECIFIED", "regions": regions}),
        );
    }
    let catalogs = catalogs_input(
        state,
        &strings(input.get("catalogs")),
        &["input", "catalogs"],
        &mut errors,
    );
    let presences = records_input(
        state,
        &strings(input.get("webPresences")),
        "MarketWebPresence",
        &["input", "webPresences"],
        "Web presence does not exist",
        "WEB_PRESENCE_NOT_FOUND",
        &mut errors,
    );
    let discounts = discounts_input(
        state,
        &strings(input.get("discounts")),
        &["input", "discounts"],
        &mut errors,
    );
    let countries: Vec<String> = regions
        .iter()
        .filter_map(|region| region.get("code")?.as_str().map(str::to_string))
        .collect();
    let kind = type_of_conditions(&conditions);
    let make_draft = input
        .get("makeDuplicateUniqueMarketsDraft")
        .or_else(|| input.get("makeDuplicateRegionMarketsDraft"))
        .and_then(Value::as_bool)
        == Some(true);
    let others = if status == "ACTIVE" && kind == "REGION" {
        duplicates(state, None, &countries)
    } else {
        Vec::new()
    };
    if !others.is_empty() && !make_draft {
        errors.push(duplicate_error(state, &others));
    }
    let id = market_gid(&handle);
    if errors.is_empty() && state.contains(&id) {
        errors.push(coded(
            &["input", "handle"],
            "Handle has already been taken",
            "TAKEN",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"market": null}), &errors));
    }
    for note in notes {
        mutation.note(format!("marketCreate: {note}"));
    }
    note_ignored(mutation, "marketCreate", &input);
    let state = mutation.state();
    let settings = currency_settings(
        state,
        input.get("currencySettings").and_then(Value::as_object),
        None,
        &regions,
    );
    let taxes_included = state
        .shop()
        .and_then(|shop| shop.get("taxesIncluded"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let inclusions = price_inclusions(
        taxes_included,
        input.get("priceInclusions").and_then(Value::as_object),
    );
    let mut record = value::object(json!({
        "__typename": "Market",
        "id": id,
        "name": name,
        "handle": handle,
        "status": status,
        "enabled": status == "ACTIVE",
        // The primary market is the store data's: a new one never is.
        "primary": false,
        "currencySettings": settings,
        "priceInclusions": inclusions,
        "delivery": {"shipping": null},
        "returnPolicyProfile": null,
        "priceList": null,
        "childMarkets": [],
        "parentMarkets": [],
        "discounts": discounts,
        "metafields": [],
    }));
    set_conditions(&mut record, conditions, regions);
    let state = mutation.state_mut();
    if make_draft {
        draft_duplicates(state, &others);
    }
    let market = state.insert(record);
    link_catalogs(state, &market, &catalogs, true);
    web_presences::link_market(state, &market, &presences, true);
    state.emit("markets/create", &market);
    Ok(payload(json!({"market": market}), &[]))
}

/// The error of a market whose countries other markets have.
fn duplicate_error(state: &ShopState, others: &[String]) -> UserError {
    let names: Vec<&str> = others
        .iter()
        .filter_map(|other| state.get(other)?.get("name")?.as_str())
        .collect();
    // lsf's wording: shopify.dev publishes the code only.
    coded(
        &["input", "conditions", "regionsCondition"],
        format!(
            "A market with the same regions already exists: {}",
            names.join(", ")
        ),
        "DUPLICATE_UNIQUE_MARKET",
    )
}

/// `marketUpdate`.
fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let state = mutation.state();
    let Some(market) = market_arg(state, ctx, "id") else {
        return Ok(payload(json!({"market": null}), &[market_not_found("id")]));
    };
    let Some(existing) = state.get(&market).cloned() else {
        return Ok(payload(json!({"market": null}), &[market_not_found("id")]));
    };
    let mut errors = Vec::new();
    let mut notes = Vec::new();
    let name = text(&input, "name").map(|name| name.trim().to_string());
    if let Some(name) = &name {
        if name.is_empty() {
            errors.push(coded(&["input", "name"], "Name can't be blank", "BLANK"));
        } else if state.keys("Market").any(|other| {
            other != market
                && state
                    .get(other)
                    .and_then(|record| record.get("name"))
                    .and_then(Value::as_str)
                    .is_some_and(|known| known.eq_ignore_ascii_case(name))
        }) {
            errors.push(coded(
                &["input", "name"],
                "Name has already been taken",
                "TAKEN",
            ));
        }
    }
    let handle = text(&input, "handle").map(handleize);
    if let Some(handle) = &handle {
        if handle.is_empty() {
            errors.push(coded(
                &["input", "handle"],
                "Handle can't be blank",
                "BLANK",
            ));
        } else if unique_handle(state, handle, Some(&market)) != *handle {
            errors.push(coded(
                &["input", "handle"],
                "Handle has already been taken",
                "TAKEN",
            ));
        }
    }
    let status = status_input(&input, &mut errors);
    let primary = existing.get("primary").and_then(Value::as_bool) == Some(true);
    if primary && status == Some("DRAFT") {
        // lsf's wording.
        errors.push(coded(
            &["input", "status"],
            "The primary market cannot be put in draft",
            "CANNOT_DISABLE_PRIMARY_MARKET",
        ));
    }
    // Conditions: those to delete, then those to add.
    let mut conditions = existing
        .get("conditions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    conditions.remove("conditionTypes");
    conditions.retain(|_, value| !value.is_null());
    let mut regions: Vec<Value> = existing
        .get("regions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let update = input
        .get("conditions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(Value::Object(delete)) = update.get("conditionsToDelete") {
        let (removed, removed_regions) = conditions_input(
            state,
            delete,
            &["input", "conditions", "conditionsToDelete"],
            &mut errors,
            &mut notes,
        );
        let codes: Vec<&Value> = removed_regions
            .iter()
            .filter_map(|region| region.get("code"))
            .collect();
        regions.retain(|region| !region.get("code").is_some_and(|code| codes.contains(&code)));
        for (name, condition) in &removed {
            if name == "regionsCondition" {
                continue;
            }
            let list = condition.as_object().and_then(|condition| {
                condition
                    .iter()
                    .find(|(key, _)| *key != "applicationLevel")
                    .map(|(key, value)| (key.clone(), strings(Some(value))))
            });
            let level = condition.get("applicationLevel").and_then(Value::as_str);
            match (conditions.get_mut(name), list) {
                (Some(Value::Object(current)), Some((key, ids))) if level == Some("SPECIFIED") => {
                    if let Some(Value::Array(items)) = current.get_mut(&key) {
                        items.retain(|item| {
                            !item
                                .as_str()
                                .is_some_and(|id| ids.iter().any(|known| known == id))
                        });
                    }
                    let empty = current
                        .get(&key)
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty);
                    let all =
                        current.get("applicationLevel").and_then(Value::as_str) == Some("ALL");
                    if empty && !all {
                        conditions.remove(name);
                    }
                }
                _ => {
                    conditions.remove(name);
                }
            }
        }
    }
    if let Some(Value::Object(add)) = update.get("conditionsToAdd") {
        let (added, added_regions) = conditions_input(
            state,
            add,
            &["input", "conditions", "conditionsToAdd"],
            &mut errors,
            &mut notes,
        );
        for region in added_regions {
            if !regions
                .iter()
                .any(|known| known.get("code") == region.get("code"))
            {
                regions.push(region);
            }
        }
        for (name, condition) in added {
            if name == "regionsCondition" {
                continue;
            }
            match (conditions.get_mut(&name), &condition) {
                (Some(Value::Object(current)), Value::Object(added))
                    if added.get("applicationLevel") == Some(&json!("SPECIFIED"))
                        && current.get("applicationLevel") == Some(&json!("SPECIFIED")) =>
                {
                    for (key, value) in added {
                        if key == "applicationLevel" {
                            continue;
                        }
                        if let Some(Value::Array(items)) = current.get_mut(key) {
                            for item in value.as_array().into_iter().flatten() {
                                if !items.contains(item) {
                                    items.push(item.clone());
                                }
                            }
                        }
                    }
                }
                _ => {
                    conditions.insert(name, condition);
                }
            }
        }
    }
    if regions.is_empty() {
        conditions.remove("regionsCondition");
    } else {
        let level = conditions
            .get("regionsCondition")
            .and_then(|condition| condition.get("applicationLevel"))
            .cloned()
            .unwrap_or(json!("SPECIFIED"));
        conditions.insert(
            "regionsCondition".into(),
            json!({"applicationLevel": level, "regions": regions}),
        );
    }
    let to_add = catalogs_input(
        state,
        &strings(input.get("catalogsToAdd")),
        &["input", "catalogsToAdd"],
        &mut errors,
    );
    let to_delete = catalogs_input(
        state,
        &strings(input.get("catalogsToDelete")),
        &["input", "catalogsToDelete"],
        &mut errors,
    );
    let presences_to_add = records_input(
        state,
        &strings(input.get("webPresencesToAdd")),
        "MarketWebPresence",
        &["input", "webPresencesToAdd"],
        "Web presence does not exist",
        "WEB_PRESENCE_NOT_FOUND",
        &mut errors,
    );
    let presences_to_delete = records_input(
        state,
        &strings(input.get("webPresencesToDelete")),
        "MarketWebPresence",
        &["input", "webPresencesToDelete"],
        "Web presence does not exist",
        "WEB_PRESENCE_NOT_FOUND",
        &mut errors,
    );
    let discounts_to_add = discounts_input(
        state,
        &strings(input.get("discountsToAdd")),
        &["input", "discountsToAdd"],
        &mut errors,
    );
    let discounts_to_delete = discounts_input(
        state,
        &strings(input.get("discountsToDelete")),
        &["input", "discountsToDelete"],
        &mut errors,
    );
    let status_now = status.unwrap_or(if is_active(&existing) {
        "ACTIVE"
    } else {
        "DRAFT"
    });
    let kind = type_of_conditions(&conditions);
    let countries: Vec<String> = regions
        .iter()
        .filter_map(|region| region.get("code")?.as_str().map(str::to_string))
        .collect();
    let make_draft = input
        .get("makeDuplicateUniqueMarketsDraft")
        .or_else(|| input.get("makeDuplicateRegionMarketsDraft"))
        .and_then(Value::as_bool)
        == Some(true);
    let others = if status_now == "ACTIVE" && kind == "REGION" {
        duplicates(state, Some(&market), &countries)
    } else {
        Vec::new()
    };
    if !others.is_empty() && !make_draft {
        errors.push(duplicate_error(state, &others));
    }
    if primary && regions.is_empty() && existing.get("regions").is_some_and(|r| r != &json!([])) {
        // lsf's wording.
        errors.push(coded(
            &["input", "conditions", "conditionsToDelete"],
            "The primary market must keep a region",
            "CANNOT_DELETE_ONLY_REGION",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"market": null}), &errors));
    }
    for note in notes {
        mutation.note(format!("marketUpdate: {note}"));
    }
    note_ignored(mutation, "marketUpdate", &input);
    let state = mutation.state();
    let remove_settings =
        input.get("removeCurrencySettings").and_then(Value::as_bool) == Some(true);
    let settings = currency_settings(
        state,
        input.get("currencySettings").and_then(Value::as_object),
        (!remove_settings)
            .then(|| existing.get("currencySettings"))
            .flatten(),
        &regions,
    );
    let taxes_included = state
        .shop()
        .and_then(|shop| shop.get("taxesIncluded"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let inclusions = if input.get("removePriceInclusions").and_then(Value::as_bool) == Some(true) {
        price_inclusions(taxes_included, None)
    } else {
        match input.get("priceInclusions").and_then(Value::as_object) {
            Some(given) => {
                let mut merged: Map<String, Value> = Map::new();
                if let Some(Value::Object(current)) = existing.get("priceInclusions") {
                    for (from, to) in [
                        ("adaptivePricingEnabled", "adaptivePricingEnabled"),
                        ("inclusiveDutiesPricingStrategy", "dutiesPricingStrategy"),
                        ("inclusiveTaxPricingStrategy", "taxPricingStrategy"),
                    ] {
                        if let Some(value) = current.get(from) {
                            merged.insert(to.into(), value.clone());
                        }
                    }
                }
                for (key, value) in given {
                    if !value.is_null() {
                        merged.insert(key.clone(), value.clone());
                    }
                }
                price_inclusions(taxes_included, Some(&merged))
            }
            None => existing
                .get("priceInclusions")
                .cloned()
                .unwrap_or_else(|| price_inclusions(taxes_included, None)),
        }
    };
    let state = mutation.state_mut();
    if make_draft {
        draft_duplicates(state, &others);
    }
    if let Some(record) = state.get_mut(&market) {
        if let Some(name) = name {
            record.insert("name".into(), json!(name));
        }
        if let Some(handle) = handle {
            record.insert("handle".into(), json!(handle));
        }
        if let Some(status) = status {
            record.insert("status".into(), json!(status));
            record.insert("enabled".into(), json!(status == "ACTIVE"));
        }
        set_conditions(record, conditions, regions);
        record.insert("currencySettings".into(), settings);
        record.insert("priceInclusions".into(), inclusions);
        if let Some(Value::Array(discounts)) = record.get_mut("discounts") {
            discounts.retain(|known| {
                !known
                    .as_str()
                    .is_some_and(|known| discounts_to_delete.iter().any(|gone| gone == known))
            });
            for discount in discounts_to_add {
                if !discounts.contains(&json!(discount)) {
                    discounts.push(json!(discount));
                }
            }
        }
    }
    link_catalogs(state, &market, &to_delete, false);
    link_catalogs(state, &market, &to_add, true);
    web_presences::link_market(state, &market, &presences_to_delete, false);
    web_presences::link_market(state, &market, &presences_to_add, true);
    state.emit("markets/update", &market);
    Ok(payload(json!({"market": market}), &[]))
}

/// `marketDelete`: the market goes, and with it its links to catalogs and web presences.
fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(market) = market_arg(state, ctx, "id") else {
        return Ok(payload(
            json!({"deletedId": null}),
            &[market_not_found("id")],
        ));
    };
    if state
        .get(&market)
        .and_then(|record| record.get("primary"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        // lsf's wording.
        let error = coded(
            &["id"],
            "The primary market cannot be deleted",
            "CANNOT_DELETE_PRIMARY_MARKET",
        );
        return Ok(payload(json!({"deletedId": null}), &[error]));
    }
    let catalogs: Vec<String> = catalogs_of(state, &market, false)
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let presences: Vec<String> = state
        .keys("MarketWebPresence")
        .map(str::to_string)
        .collect();
    let metafields = crate::domains::catalog::gids(
        state
            .get(&market)
            .and_then(|record| record.get("metafields")),
    );
    let state = mutation.state_mut();
    link_catalogs(state, &market, &catalogs, false);
    web_presences::link_market(state, &market, &presences, false);
    for metafield in metafields {
        crate::domains::content::metafields::remove(state, &metafield);
    }
    state.delete(&market, "markets/delete");
    Ok(payload(json!({"deletedId": market}), &[]))
}

/// `marketCurrencySettingsUpdate` (deprecated): the currency settings of a market.
fn currency_settings_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(market) = market_arg(state, ctx, "marketId") else {
        return Ok(payload(
            json!({"market": null}),
            &[market_not_found("marketId")],
        ));
    };
    let input = ctx.arg_object_or_default("input");
    let existing = state.get(&market).cloned().unwrap_or_default();
    let regions: Vec<Value> = existing
        .get("regions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if input.get("localCurrencies").and_then(Value::as_bool) == Some(true) && regions.len() <= 1 {
        // The code's description in the schema: "Local currencies can't be enabled on a
        // single-country market" is lsf's shortening of it.
        let error = coded(
            &["input", "localCurrencies"],
            "Local currencies can't be enabled on a single-country market",
            "NO_LOCAL_CURRENCIES_ON_SINGLE_COUNTRY_MARKET",
        );
        return Ok(payload(json!({"market": null}), &[error]));
    }
    let settings = currency_settings(
        state,
        Some(&input),
        existing.get("currencySettings"),
        &regions,
    );
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&market) {
        record.insert("currencySettings".into(), settings);
    }
    state.emit("markets/update", &market);
    Ok(payload(json!({"market": market}), &[]))
}

/// Adds or removes regions of a market: what `marketRegionsCreate` and `marketRegionsDelete`
/// share, `markets/update` for each market changed.
fn change_regions(state: &mut ShopState, market: &str, add: &[Value], remove: &[String]) {
    let Some(record) = state.get(market).cloned() else {
        return;
    };
    let mut regions: Vec<Value> = record
        .get("regions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    regions.retain(|region| {
        !region
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| remove.iter().any(|gone| gone == id))
    });
    for region in add {
        if !regions
            .iter()
            .any(|known| known.get("code") == region.get("code"))
        {
            regions.push(region.clone());
        }
    }
    let mut conditions = record
        .get("conditions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    conditions.remove("conditionTypes");
    conditions.retain(|_, value| !value.is_null());
    if regions.is_empty() {
        conditions.remove("regionsCondition");
    } else {
        conditions.insert(
            "regionsCondition".into(),
            json!({"applicationLevel": "SPECIFIED", "regions": regions}),
        );
    }
    if let Some(record) = state.get_mut(market) {
        set_conditions(record, conditions, regions);
    }
    state.emit("markets/update", market);
}

/// `marketRegionsCreate` (deprecated).
fn regions_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(market) = market_arg(state, ctx, "marketId") else {
        return Ok(payload(
            json!({"market": null}),
            &[market_not_found("marketId")],
        ));
    };
    let regions: Vec<Value> = objects(ctx.arg("regions"))
        .iter()
        .filter_map(|region| text(region, "countryCode"))
        .map(|code| region(state, code))
        .collect();
    let countries: Vec<String> = regions
        .iter()
        .filter_map(|region| region.get("code")?.as_str().map(str::to_string))
        .collect();
    let active = state.get(&market).is_some_and(is_active);
    let others = if active {
        duplicates(state, Some(&market), &countries)
    } else {
        Vec::new()
    };
    if !others.is_empty() {
        let mut error = duplicate_error(state, &others);
        error.field = Some(vec!["regions".to_string()]);
        return Ok(payload(json!({"market": null}), &[error]));
    }
    change_regions(mutation.state_mut(), &market, &regions, &[]);
    Ok(payload(json!({"market": market}), &[]))
}

/// The market that has a region, by the region's GID.
fn market_of_region(state: &ShopState, region: &str) -> Option<String> {
    state
        .keys("Market")
        .find(|market| {
            state.get(market).is_some_and(|record| {
                record
                    .get("regions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|known| known.get("id").and_then(Value::as_str) == Some(region))
            })
        })
        .map(str::to_string)
}

/// Removes regions by their GIDs: what `marketRegionDelete` and `marketRegionsDelete` share.
/// The regions removed, or the errors.
fn delete_regions(
    mutation: &mut Mutation<'_>,
    ids: &[String],
    field: &str,
    indexed: bool,
) -> Result<Vec<(String, String)>, Vec<UserError>> {
    let state = mutation.state();
    let mut found = Vec::new();
    let mut errors = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let index = index.to_string();
        let at: Vec<&str> = if indexed {
            vec![field, &index]
        } else {
            vec![field]
        };
        match market_of_region(state, id) {
            None => errors.push(coded(&at, "Region does not exist", "REGION_NOT_FOUND")),
            Some(market) => {
                let record = state.get(&market);
                let left = record.map_or(0, |record| {
                    regions_of(record).len()
                        - ids
                            .iter()
                            .filter(|gone| {
                                record
                                    .get("regions")
                                    .and_then(Value::as_array)
                                    .into_iter()
                                    .flatten()
                                    .any(|region| region.get("id") == Some(&json!(gone)))
                            })
                            .count()
                });
                if left == 0 {
                    // The code's description in the schema.
                    errors.push(coded(
                        &at,
                        "Can't delete the only region in a market.",
                        "CANNOT_DELETE_ONLY_REGION",
                    ));
                } else {
                    found.push((market, id.clone()));
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let state = mutation.state_mut();
    let mut markets: Vec<String> = found.iter().map(|(market, _)| market.clone()).collect();
    markets.dedup();
    for market in markets {
        let gone: Vec<String> = found
            .iter()
            .filter(|(of, _)| *of == market)
            .map(|(_, id)| id.clone())
            .collect();
        change_regions(state, &market, &[], &gone);
    }
    Ok(found)
}

/// `marketRegionDelete` (deprecated).
fn region_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    match delete_regions(mutation, std::slice::from_ref(&id), "id", false) {
        Ok(found) => {
            let market = found.first().map(|(market, _)| market.clone());
            Ok(payload(json!({"deletedId": id, "market": market}), &[]))
        }
        Err(errors) => Ok(payload(json!({"deletedId": null, "market": null}), &errors)),
    }
}

/// `marketRegionsDelete` (deprecated).
fn regions_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let ids = strings(ctx.arg("ids"));
    match delete_regions(mutation, &ids, "ids", true) {
        Ok(found) => {
            let deleted: Vec<String> = found.into_iter().map(|(_, id)| id).collect();
            Ok(payload(json!({"deletedIds": deleted}), &[]))
        }
        Err(errors) => Ok(payload(json!({"deletedIds": null}), &errors)),
    }
}

/// `backupRegionUpdate`: the region used "when we have no better signal of what region a buyer
/// is in". `null` sets it to the shop's country.
fn backup_region_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let code = ctx
        .arg("region")
        .and_then(|region| region.get("countryCode"))
        .and_then(Value::as_str)
        .map(str::to_ascii_uppercase)
        .or_else(|| {
            state
                .shop()
                .and_then(|shop| shop.pointer_str("/shopAddress/countryCodeV2"))
                .map(str::to_string)
        })
        .unwrap_or_else(|| "US".to_string());
    // The backup region is one the shop sells to: a region of one of its markets.
    if !state
        .all("Market")
        .any(|market| regions_of(market).contains(&code))
    {
        // lsf's wording.
        let error = coded(
            &["region", "countryCode"],
            "The backup region must be a region of a market",
            "REGION_NOT_FOUND",
        );
        return Ok(payload(json!({"backupRegion": null}), &[error]));
    }
    let backup = region(state, &code);
    let changed = state.setting(super::BACKUP_REGION) != Some(&json!(code));
    let state = mutation.state_mut();
    if changed {
        state.set_setting(super::BACKUP_REGION, json!(code));
        let mut snapshot = Record::new();
        snapshot.insert("__typename".into(), json!("MarketRegionCountry"));
        snapshot.insert("id".into(), backup.get("id").cloned().unwrap_or_default());
        snapshot.insert("code".into(), json!(code));
        // The backup region is no record: the event's key names none, its snapshot is it.
        state.emit_with("markets_backup_region/update", "backup-region", snapshot);
    }
    Ok(payload(json!({"backupRegion": backup}), &[]))
}
