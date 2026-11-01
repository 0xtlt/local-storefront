//! Price lists: `priceListCreate`, `priceListUpdate`, `priceListDelete`, their fixed prices
//! (`priceListFixedPricesAdd`, `priceListFixedPricesUpdate`, `priceListFixedPricesDelete`,
//! `priceListFixedPricesByProductUpdate`), quantity rules (`quantityRulesAdd`,
//! `quantityRulesDelete`) and quantity price breaks (`quantityPricingByVariantUpdate`); the
//! `prices` and `quantityRules` of a price list, and the `contextualPricing(context:)` of
//! products and variants that they give.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A price list has a unique name, a currency, and a parent adjustment: a percentage up or
//!   down of the variants' prices (`PERCENTAGE_INCREASE`, `PERCENTAGE_DECREASE`), with the
//!   compare-at prices adjusted too (`ADJUSTED`) or removed (`NULLIFY`). Those are its
//!   `RELATIVE` prices; a fixed price (`FIXED`) overrides one variant's, in the price list's
//!   currency ("The specified currency does not match the price list's currency.",
//!   `PRICE_LIST_CURRENCY_MISMATCH`, the reference's example). "Changing the currency removes
//!   all fixed prices from the price list."
//! - Adding a fixed price "replaces any existing fixed prices for those variants"; deleting
//!   reverts the variant "to the original price that was determined by the price list
//!   adjustment". `deletedFixedPriceVariantIds` is in the order of the ids (the example).
//! - Quantity rules (minimum, maximum, increment) of variants, checked as the example of
//!   `quantityRulesAdd` answers: the increment at most the minimum, the minimum and the maximum
//!   multiples of the increment, the minimum at most the maximum; a rule of 1, none and 1
//!   `isDefault`. Quantity rules and price breaks are B2B: a market catalog's price list takes
//!   none (`CATALOG_CONTEXT_DOES_NOT_SUPPORT_QUANTITY_RULES`, `…_QUANTITY_PRICE_BREAKS`).
//! - `quantityPricingByVariantUpdate` "executes delete operations before create operations and
//!   doesn't allow partial updates": fixed prices, then rules, then price breaks, which need a
//!   fixed price of the variant on the price list and fit its rule (the messages of its
//!   example, Shopify's typo "Miniumum" included).
//! - Contextual pricing: the prices a buyer gets in a context (a country, a company location, a
//!   location): from the price list of the context's catalog, the lowest when several apply, as
//!   B2B does ("Manage catalogs"); without one, the variant's price in the market's currency.
//!
//! lsf has no exchange rates: a price in another currency than the shop's is converted with the
//! market's `baseCurrencyManualRate` when it has one, else at 1.0, which `extensions.lsf.notes`
//! says. Shopify has no webhook topic for price lists: their changes are `price_lists/create`,
//! `price_lists/update` and `price_lists/delete` events, which no subscription can name. The
//! messages are lsf's where shopify.dev publishes none (each says so).

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use super::{
    Pointer, coded, input, is, market_currency, number, objects, shop_currency, strings, text,
};
use crate::domains::catalog::{cents_of, gids};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::search::SearchValue;
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("priceListCreate", create)
        .mutation("priceListUpdate", update)
        .mutation("priceListDelete", delete)
        .mutation("priceListFixedPricesAdd", fixed_prices_add)
        .mutation("priceListFixedPricesUpdate", fixed_prices_update)
        .mutation("priceListFixedPricesDelete", fixed_prices_delete)
        .mutation(
            "priceListFixedPricesByProductUpdate",
            fixed_prices_by_product_update,
        )
        .mutation("quantityRulesAdd", quantity_rules_add)
        .mutation("quantityRulesDelete", quantity_rules_delete)
        .mutation("quantityPricingByVariantUpdate", quantity_pricing_update)
        .field("PriceList", "prices", prices)
        .field("PriceList", "quantityRules", quantity_rules)
        .field(
            "PriceListPrice",
            "quantityPriceBreaks",
            price_breaks_of_price,
        )
        .field(
            "ProductVariant",
            "contextualPricing",
            variant_contextual_pricing,
        )
        .field("Product", "contextualPricing", product_contextual_pricing)
        .search_field("PriceListPrice", "variant_id", |_r, _item, price| {
            id_value(price.get("variant"))
        })
        .search_field("PriceListPrice", "product_id", |r, _item, price| {
            let variant = price.get("variant")?.as_str()?;
            id_value(r.state().get(variant)?.get("product"))
        });
}

fn id_value(gid: Option<&Value>) -> Option<Vec<SearchValue>> {
    let (_, id) = crate::state::parse_gid(gid?.as_str()?)?;
    Some(vec![SearchValue::Id(id)])
}

// --- Prices ---

/// A price list's adjustment as a factor in basis points: 10% up is 11000, 10% down 9000.
fn factor_bps(list: &Record) -> i64 {
    let adjustment = list
        .get("parent")
        .and_then(|parent| parent.get("adjustment"));
    let percent = adjustment
        .and_then(|adjustment| adjustment.get("value"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let bps = (percent * 100.0).round() as i64;
    match adjustment
        .and_then(|adjustment| adjustment.get("type"))
        .and_then(Value::as_str)
    {
        Some("PERCENTAGE_INCREASE") => 10_000 + bps,
        _ => 10_000 - bps,
    }
}

/// `cents × bps / 10000`, rounded half away from zero.
fn scale(cents: i64, bps: i64) -> i64 {
    let product = i128::from(cents) * i128::from(bps);
    let rounded = (product + product.signum() * 5_000) / 10_000;
    i64::try_from(rounded).unwrap_or(i64::MAX)
}

/// The rate from the shop's currency to `currency`, in basis points: 1.0 for the shop's own,
/// else the manual rate of a market that sells in it. `None` when lsf has no rate (1.0 is
/// used). What every conversion of lsf uses: prices of price lists, contextual prices, the
/// orders a subscription bills in its currency.
pub fn rate_bps(state: &ShopState, currency: &str) -> Option<i64> {
    if currency == shop_currency(state) {
        return Some(10_000);
    }
    state.all("Market").find_map(|market| {
        let base = market.get("currencySettings")?.get("baseCurrency")?;
        if base.get("currencyCode")?.as_str()? != currency {
            return None;
        }
        let rate: f64 = match base.get("manualRate")? {
            Value::String(text) => text.parse().ok()?,
            Value::Number(number) => number.as_f64()?,
            _ => return None,
        };
        Some((rate * 10_000.0).round() as i64)
    })
}

/// A variant's price and compare-at price in the shop's currency, in cents.
fn variant_cents(state: &ShopState, variant: &str) -> (i64, Option<i64>) {
    let record = state.get(variant);
    let price = cents_of(record.and_then(|record| record.get("price"))).unwrap_or(0);
    let compare = cents_of(record.and_then(|record| record.get("compareAtPrice")));
    (price, compare)
}

/// The fixed price of a variant on a price list: `(price, compare-at)` in cents.
fn fixed_of(list: &Record, variant: &str) -> Option<(i64, Option<i64>)> {
    list.get("lsfFixedPrices")
        .and_then(Value::as_array)?
        .iter()
        .find(|price| price.get("variant").and_then(Value::as_str) == Some(variant))
        .map(|price| {
            (
                cents_of(price.pointer("/price/amount")).unwrap_or(0),
                cents_of(price.pointer("/compareAtPrice/amount")),
            )
        })
}

/// A variant's price on a price list: `(price, compare-at, FIXED or RELATIVE, converted)` in
/// cents of the list's currency; `converted` is false when lsf had no rate.
fn list_price(
    state: &ShopState,
    list: &Record,
    variant: &str,
) -> (i64, Option<i64>, &'static str, bool) {
    if let Some((price, compare)) = fixed_of(list, variant) {
        return (price, compare, "FIXED", true);
    }
    let currency = list
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let rate = rate_bps(state, currency);
    let (price, compare) = variant_cents(state, variant);
    let factor = factor_bps(list);
    let convert = |cents: i64| scale(scale(cents, rate.unwrap_or(10_000)), factor);
    let nullify = list
        .get("parent")
        .and_then(|parent| parent.pointer("/settings/compareAtMode"))
        .and_then(Value::as_str)
        == Some("NULLIFY");
    let compare = if nullify { None } else { compare.map(convert) };
    (convert(price), compare, "RELATIVE", rate.is_some())
}

/// Every variant of the shop, in the order of the products and of their variants.
fn all_variants(state: &ShopState) -> Vec<String> {
    state
        .all("Product")
        .flat_map(|product| gids(product.get("variants")))
        .filter(|variant| is(state, variant, "ProductVariant"))
        .collect()
}

/// The price breaks of a variant on a price list.
fn breaks_of(state: &ShopState, list: &str, variant: &str) -> Vec<String> {
    state
        .get(list)
        .map(|record| gids(record.get("lsfPriceBreaks")))
        .unwrap_or_default()
        .into_iter()
        .filter(|price_break| {
            state
                .get(price_break)
                .and_then(|record| record.get("variant"))
                .and_then(Value::as_str)
                == Some(variant)
        })
        .collect()
}

/// The `PriceListPrice` of a variant on a price list, inline.
fn price_object(state: &ShopState, key: &str, list: &Record, variant: &str) -> Value {
    let currency = list
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (price, compare, origin, _) = list_price(state, list, variant);
    json!({
        "variant": variant,
        "price": value::money_v2(price, currency),
        "compareAtPrice": compare.map(|compare| value::money_v2(compare, currency)),
        "originType": origin,
        "quantityPriceBreaks": breaks_of(state, key, variant),
    })
}

/// `PriceList.prices(originType:, query:)`: every variant's price, fixed or relative.
fn prices(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let key = object.key().unwrap_or_default();
    let Some(list) = state.get(key) else {
        return r.connection(ctx, Vec::new());
    };
    let origin = ctx.arg_str("originType");
    let items: Vec<Value> = all_variants(state)
        .iter()
        .map(|variant| price_object(state, key, list, variant))
        .filter(|price| origin.is_none_or(|origin| price["originType"] == json!(origin)))
        .collect();
    let currency = list
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if rate_bps(state, currency).is_none() {
        r.note(format!(
            "PriceList.prices: lsf has no exchange rate from {} to {currency}: relative prices are converted at 1.0",
            shop_currency(state)
        ));
    }
    r.connection(ctx, items)
}

/// `PriceListPrice.quantityPriceBreaks`: the records it names.
fn price_breaks_of_price(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, r.items_of(object, "quantityPriceBreaks"))
}

/// A `QuantityRule` inline: a variant's rule of a price list, or the default one.
fn rule_object(variant: &str, rule: Option<&Value>) -> Value {
    match rule {
        Some(rule) => {
            let minimum = rule.get("minimum").and_then(Value::as_i64).unwrap_or(1);
            let maximum = rule.get("maximum").and_then(Value::as_i64);
            let increment = rule.get("increment").and_then(Value::as_i64).unwrap_or(1);
            json!({
                "productVariant": variant,
                "minimum": minimum,
                "maximum": maximum,
                "increment": increment,
                "isDefault": minimum == 1 && maximum.is_none() && increment == 1,
                "originType": "FIXED",
            })
        }
        None => json!({
            "productVariant": variant,
            "minimum": 1,
            "maximum": null,
            "increment": 1,
            "isDefault": true,
            "originType": "RELATIVE",
        }),
    }
}

/// The rule of a variant on a price list, as kept.
fn rule_of<'r>(list: &'r Record, variant: &str) -> Option<&'r Value> {
    list.get("lsfQuantityRules")
        .and_then(Value::as_array)?
        .iter()
        .find(|rule| rule.get("variant").and_then(Value::as_str) == Some(variant))
}

/// `PriceList.quantityRules(originType:)`: the rules set on it (`FIXED`).
fn quantity_rules(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let Some(list) = object.key().and_then(|key| state.get(key)) else {
        return r.connection(ctx, Vec::new());
    };
    if ctx.arg_str("originType") == Some("RELATIVE") {
        return r.connection(ctx, Vec::new());
    }
    let items = list
        .get("lsfQuantityRules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|rule| {
            let variant = rule.get("variant")?.as_str()?;
            Some(rule_object(variant, Some(rule)))
        })
        .collect();
    r.connection(ctx, items)
}

// --- Contextual pricing ---

/// The price a buyer gets for a variant in a context.
pub struct ContextualPrice {
    pub cents: i64,
    pub compare_at: Option<i64>,
    pub currency: String,
    /// The price list it comes from.
    pub price_list: Option<String>,
    /// Whether lsf had the rate it needed (else 1.0 was used).
    pub converted: bool,
}

/// The price lists of a context (`{country, companyLocationId, locationId}`): those of the
/// active catalogs of the company location, else of its market; of the location's market; of
/// the country's market. Also the market, for the currency without a price list.
fn lists_of_context(
    state: &ShopState,
    context: &Map<String, Value>,
) -> (Vec<String>, Option<String>) {
    let list_of = |catalog: &str| -> Option<String> {
        let record = state.get(catalog)?;
        (record.get("status").and_then(Value::as_str) == Some("ACTIVE"))
            .then(|| record.get("priceList")?.as_str().map(str::to_string))
            .flatten()
            .filter(|list| state.contains(list))
    };
    let market_lists = |market: &str| -> Vec<String> {
        super::market::catalogs_of(state, market, true)
            .iter()
            .filter_map(Value::as_str)
            .filter_map(list_of)
            .collect()
    };
    if let Some(location) = text(context, "companyLocationId") {
        let lists: Vec<String> = super::catalogs::of_location(state, location)
            .iter()
            .filter_map(Value::as_str)
            .filter_map(list_of)
            .collect();
        let market = super::catalogs::market_of_location(state, location);
        if !lists.is_empty() {
            return (lists, market);
        }
        if let Some(market) = market {
            return (market_lists(&market), Some(market));
        }
        return (Vec::new(), None);
    }
    if let Some(location) = text(context, "locationId") {
        let market = state.keys("Market").find(|market| {
            state.get(market).is_some_and(|record| {
                super::is_active(record)
                    && record
                        .get("conditions")
                        .and_then(|conditions| conditions.get("locationsCondition"))
                        .is_some_and(|condition| {
                            condition.get("applicationLevel") == Some(&json!("ALL"))
                                || strings(condition.get("locations"))
                                    .iter()
                                    .any(|known| known == location)
                        })
            })
        });
        if let Some(market) = market {
            return (market_lists(market), Some(market.to_string()));
        }
    }
    if let Some(country) = text(context, "country")
        && let Some(market) = super::market_of_country(state, country)
    {
        return (market_lists(&market), Some(market));
    }
    (Vec::new(), None)
}

/// The price of a variant in a context: the lowest of the context's price lists, else the
/// variant's own in the market's currency. Shared with the sync's reading of B2B prices.
pub fn contextual_price(
    state: &ShopState,
    variant: &str,
    context: &Map<String, Value>,
) -> ContextualPrice {
    let (lists, market) = lists_of_context(state, context);
    let best = lists
        .iter()
        .filter_map(|list| Some((list, state.get(list)?)))
        .map(|(key, list)| (key, list, list_price(state, list, variant)))
        .min_by_key(|(_, _, (price, _, _, _))| *price);
    if let Some((key, list, (cents, compare_at, _, converted))) = best {
        return ContextualPrice {
            cents,
            compare_at,
            currency: list
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            price_list: Some(key.clone()),
            converted,
        };
    }
    let currency = market.as_deref().map_or_else(
        || shop_currency(state),
        |market| market_currency(state, market),
    );
    let rate = rate_bps(state, &currency);
    let (price, compare) = variant_cents(state, variant);
    let convert = |cents: i64| scale(cents, rate.unwrap_or(10_000));
    ContextualPrice {
        cents: convert(price),
        compare_at: compare.map(convert),
        currency,
        price_list: None,
        converted: rate.is_some(),
    }
}

/// The `ProductVariantContextualPricing` of a variant in a context, built whole.
fn variant_pricing(
    state: &ShopState,
    variant: &str,
    context: &Map<String, Value>,
) -> (Record, bool) {
    let price = contextual_price(state, variant, context);
    let list = price.price_list.as_deref().and_then(|list| state.get(list));
    let rule = list.and_then(|list| rule_of(list, variant));
    let breaks = price
        .price_list
        .as_deref()
        .map(|list| breaks_of(state, list, variant))
        .unwrap_or_default();
    let unit_price = state
        .get(variant)
        .and_then(|record| record.get("unitPrice"))
        .cloned()
        .unwrap_or(Value::Null);
    let pricing = value::object(json!({
        "price": value::money_v2(price.cents, &price.currency),
        "compareAtPrice": price
            .compare_at
            .map(|compare| value::money_v2(compare, &price.currency)),
        "unitPrice": unit_price,
        "quantityRule": rule_object(variant, rule),
        "quantityPriceBreaks": breaks,
        "auditTrail": {"priceAdjustments": []},
    }));
    (pricing, price.converted)
}

/// Notes a price converted at 1.0.
fn note_rate(r: &ShopResolver<'_>, converted: bool) {
    if !converted {
        r.note(format!(
            "contextualPricing: lsf has no exchange rates: a price in another currency than {} is converted at 1.0 (or at the market's baseCurrencyManualRate)",
            shop_currency(r.state())
        ));
    }
}

fn context_arg(ctx: &FieldContext<'_>) -> Map<String, Value> {
    input(ctx, "context")
}

fn variant_contextual_pricing(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let variant = object.key().unwrap_or_default();
    let (pricing, converted) = variant_pricing(r.state(), variant, &context_arg(ctx));
    note_rate(r, converted);
    Ok(Output::Object(Obj::built(
        "ProductVariantContextualPricing",
        pricing,
    )))
}

fn product_contextual_pricing(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let product = object.key().unwrap_or_default();
    let context = context_arg(ctx);
    let variants: Vec<String> = state
        .get(product)
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default();
    let mut priced: Vec<(i64, Record)> = Vec::new();
    let mut converted = true;
    let mut fixed_rules = 0;
    for variant in &variants {
        let (pricing, ok) = variant_pricing(state, variant, &context);
        converted &= ok;
        if pricing
            .get("quantityRule")
            .and_then(|rule| rule.get("originType"))
            == Some(&json!("FIXED"))
        {
            fixed_rules += 1;
        }
        let cents =
            cents_of(pricing.get("price").and_then(|price| price.get("amount"))).unwrap_or(0);
        priced.push((cents, pricing));
    }
    note_rate(r, converted);
    let min = priced
        .iter()
        .min_by_key(|(cents, _)| *cents)
        .map(|(_, p)| p.clone());
    let max = priced
        .iter()
        .max_by_key(|(cents, _)| *cents)
        .map(|(_, p)| p.clone());
    let currency = min
        .as_ref()
        .and_then(|pricing| pricing.pointer_str("/price/currencyCode"))
        .map_or_else(|| shop_currency(state), str::to_string);
    let price_of = |pricing: &Option<Record>| {
        pricing
            .as_ref()
            .and_then(|pricing| pricing.get("price").cloned())
            .unwrap_or_else(|| value::money_v2(0, &currency))
    };
    let mut value = Record::new();
    value.insert("fixedQuantityRulesCount".into(), json!(fixed_rules));
    value.insert(
        "priceRange".into(),
        json!({"minVariantPrice": price_of(&min), "maxVariantPrice": price_of(&max)}),
    );
    value.insert("minVariantPricing".into(), json!(min));
    value.insert("maxVariantPricing".into(), json!(max));
    Ok(Output::Object(Obj::built(
        "ProductContextualPricing",
        value,
    )))
}

// --- What the mutations share ---

/// The price list of an argument.
fn list_arg(state: &ShopState, ctx: &FieldContext<'_>, name: &str) -> Option<String> {
    let id = ctx.arg_str(name)?;
    is(state, id, "PriceList").then(|| id.to_string())
}

/// Recomputes `fixedPricesCount`.
fn refresh(state: &mut ShopState, list: &str) {
    let count = state
        .get(list)
        .and_then(|record| record.get("lsfFixedPrices"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if state
        .get(list)
        .and_then(|record| record.get("fixedPricesCount"))
        != Some(&json!(count))
        && let Some(record) = state.get_mut(list)
    {
        record.insert("fixedPricesCount".into(), json!(count));
    }
}

/// A `MoneyInput` in cents, and its currency.
fn money(input: &Map<String, Value>) -> Option<(i64, String)> {
    let cents = cents_of(input.get("amount"))?;
    let currency = input.get("currencyCode")?.as_str()?.to_string();
    Some((cents, currency))
}

/// The currency mismatch of a price input, Shopify's example error.
fn mismatch(field: &[&str], code: &str) -> UserError {
    coded(
        field,
        "The specified currency does not match the price list's currency.",
        code,
    )
}

/// A fixed price input, checked: `(variant, price, compare-at)`.
struct FixedPrice {
    variant: String,
    price: Value,
    compare_at: Value,
}

/// Checks `[PriceListPriceInput!]` at `at` for a price list of `currency`.
fn check_prices(
    state: &ShopState,
    inputs: &[Map<String, Value>],
    at: &[&str],
    currency: &str,
    codes: (&str, &str),
    errors: &mut Vec<UserError>,
) -> Vec<FixedPrice> {
    let (variant_code, currency_code) = codes;
    let mut found = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let index = index.to_string();
        let path = |field: &[&str]| -> Vec<String> {
            at.iter()
                .chain([index.as_str()].iter())
                .chain(field.iter())
                .map(|part| part.to_string())
                .collect()
        };
        let error = |field: &[&str], message: &str, code: &str| UserError {
            field: Some(path(field)),
            message: message.to_string(),
            code: Some(code.to_string()),
        };
        let variant = text(input, "variantId").unwrap_or_default();
        let mut ok = true;
        if !is(state, variant, "ProductVariant") {
            // The code's description in the schema.
            errors.push(error(&["variantId"], "Variant not found.", variant_code));
            ok = false;
        }
        let price = input
            .get("price")
            .and_then(Value::as_object)
            .and_then(money);
        if let Some((_, given)) = &price
            && given != currency
        {
            errors.push(UserError {
                field: Some(path(&["price", "currencyCode"])),
                ..mismatch(&[], currency_code)
            });
            ok = false;
        }
        let compare = input
            .get("compareAtPrice")
            .and_then(Value::as_object)
            .and_then(money);
        if let Some((_, given)) = &compare
            && given != currency
        {
            errors.push(UserError {
                field: Some(path(&["compareAtPrice", "currencyCode"])),
                ..mismatch(&[], currency_code)
            });
            ok = false;
        }
        if ok && let Some((cents, _)) = price {
            found.push(FixedPrice {
                variant: variant.to_string(),
                price: value::money_v2(cents, currency),
                compare_at: compare
                    .map(|(cents, _)| value::money_v2(cents, currency))
                    .unwrap_or(Value::Null),
            });
        }
    }
    found
}

/// Sets fixed prices on a price list, replacing those of the same variants.
fn set_fixed(state: &mut ShopState, list: &str, prices: &[FixedPrice]) {
    if let Some(record) = state.get_mut(list) {
        let entry = record.entry("lsfFixedPrices").or_insert_with(|| json!([]));
        if let Value::Array(items) = entry {
            for price in prices {
                items.retain(|known| known.get("variant") != Some(&json!(price.variant)));
                items.push(json!({
                    "variant": price.variant,
                    "price": price.price,
                    "compareAtPrice": price.compare_at,
                }));
            }
        }
    }
    refresh(state, list);
}

/// Removes the fixed prices of variants from a price list, and their price breaks.
fn remove_fixed(state: &mut ShopState, list: &str, variants: &[String]) {
    if let Some(Value::Array(items)) = state
        .get_mut(list)
        .and_then(|record| record.get_mut("lsfFixedPrices"))
    {
        items.retain(|known| {
            !known
                .get("variant")
                .and_then(Value::as_str)
                .is_some_and(|variant| variants.iter().any(|gone| gone == variant))
        });
    }
    for variant in variants {
        for price_break in breaks_of(state, list, variant) {
            remove_break(state, list, &price_break);
        }
    }
    refresh(state, list);
}

/// The variants with a fixed price on a price list.
fn fixed_variants(state: &ShopState, list: &str) -> HashSet<String> {
    state
        .get(list)
        .and_then(|record| record.get("lsfFixedPrices"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|price| price.get("variant")?.as_str().map(str::to_string))
        .collect()
}

/// The `PriceListPrice`s of variants, as the payloads answer them.
fn prices_of(state: &ShopState, list: &str, variants: &[String]) -> Vec<Value> {
    let Some(record) = state.get(list) else {
        return Vec::new();
    };
    variants
        .iter()
        .map(|variant| price_object(state, list, record, variant))
        .collect()
}

fn list_not_found(field: &str, code: &str) -> UserError {
    // The code's description in the schema.
    coded(&[field], "Price list not found.", code)
}

/// Whether a price list's catalog takes quantity rules and price breaks: a company location
/// catalog's, or a price list of no catalog yet.
fn supports_quantity(state: &ShopState, list: &str) -> bool {
    state
        .get(list)
        .and_then(|record| record.get("catalog"))
        .and_then(Value::as_str)
        .and_then(|catalog| state.type_of(catalog))
        .is_none_or(|type_name| type_name == "CompanyLocationCatalog")
}

/// What deleting variants runs: the price lists no longer hold their fixed prices, quantity
/// rules nor price breaks, and count their fixed prices again (`fixedPricesCount`), with
/// `price_lists/update` for each list that changed. Nothing for a shop without price lists.
pub fn removing_variants(state: &mut ShopState, variants: &[String]) {
    let gone = |variant: Option<&Value>| {
        variant
            .and_then(Value::as_str)
            .is_some_and(|variant| variants.iter().any(|known| known == variant))
    };
    let lists: Vec<String> = state.keys("PriceList").map(str::to_string).collect();
    for list in lists {
        let Some(record) = state.get(&list) else {
            continue;
        };
        let holds = |field: &str| {
            record
                .get(field)
                .and_then(Value::as_array)
                .is_some_and(|entries| entries.iter().any(|entry| gone(entry.get("variant"))))
        };
        let breaks: Vec<String> = gids(record.get("lsfPriceBreaks"))
            .into_iter()
            .filter(|price_break| {
                state
                    .get(price_break)
                    .is_some_and(|known| gone(known.get("variant")))
            })
            .collect();
        if !holds("lsfFixedPrices") && !holds("lsfQuantityRules") && breaks.is_empty() {
            continue;
        }
        if let Some(record) = state.get_mut(&list) {
            for field in ["lsfFixedPrices", "lsfQuantityRules"] {
                if let Some(Value::Array(entries)) = record.get_mut(field) {
                    entries.retain(|entry| !gone(entry.get("variant")));
                }
            }
            if let Some(Value::Array(listed)) = record.get_mut("lsfPriceBreaks") {
                listed.retain(|known| {
                    !known
                        .as_str()
                        .is_some_and(|id| breaks.iter().any(|gone| gone == id))
                });
            }
        }
        for price_break in &breaks {
            state.remove(price_break);
        }
        refresh(state, &list);
        state.touch(&list);
        state.emit("price_lists/update", &list);
    }
}

/// Removes a price list, its price breaks, and its catalog's link to it.
pub fn delete_price_list(state: &mut ShopState, list: &str) {
    let Some(record) = state.get(list).cloned() else {
        return;
    };
    for price_break in gids(record.get("lsfPriceBreaks")) {
        state.remove(&price_break);
    }
    if let Some(catalog) = record.get("catalog").and_then(Value::as_str)
        && let Some(catalog) = state.get_mut(catalog)
        && catalog.get("priceList") == Some(&json!(list))
    {
        catalog.insert("priceList".into(), Value::Null);
    }
    state.delete(list, "price_lists/delete");
}

// --- Price lists ---

/// Checks a parent adjustment (`at`: its path).
fn check_parent(parent: &Map<String, Value>, at: &[&str], errors: &mut Vec<UserError>) {
    let Some(adjustment) = parent.get("adjustment").and_then(Value::as_object) else {
        return;
    };
    let value = adjustment
        .get("value")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let decrease = text(adjustment, "type") == Some("PERCENTAGE_DECREASE");
    let mut field: Vec<&str> = at.to_vec();
    field.extend(["adjustment", "value"]);
    // lsf's bounds and wording: a decrease of more than 100% would make prices negative;
    // shopify.dev documents the codes only.
    if value < 0.0 {
        errors.push(coded(
            &field,
            "Adjustment value must be greater than or equal to 0",
            "INVALID_ADJUSTMENT_MIN_VALUE",
        ));
    } else if decrease && value > 100.0 {
        errors.push(coded(
            &field,
            "Adjustment value must be less than or equal to 100",
            "INVALID_ADJUSTMENT_MAX_VALUE",
        ));
    } else if value > 1000.0 {
        errors.push(coded(
            &field,
            "Adjustment value must be less than or equal to 1000",
            "INVALID_ADJUSTMENT_MAX_VALUE",
        ));
    }
}

/// A `PriceListParent` from its input.
fn parent_of(parent: &Map<String, Value>) -> Value {
    let adjustment = parent.get("adjustment").and_then(Value::as_object);
    json!({
        "adjustment": {
            "type": adjustment.and_then(|adjustment| text(adjustment, "type")).unwrap_or("PERCENTAGE_DECREASE"),
            "value": adjustment.and_then(|adjustment| adjustment.get("value")).and_then(Value::as_f64).unwrap_or(0.0),
        },
        "settings": {
            "compareAtMode": parent
                .get("settings")
                .and_then(|settings| settings.get("compareAtMode"))
                .and_then(Value::as_str)
                .unwrap_or("ADJUSTED"),
        },
    })
}

/// Checks the catalog of a price list input: `(catalog)`.
fn check_catalog(
    state: &ShopState,
    input: &Map<String, Value>,
    list: Option<&str>,
    currency: &str,
    errors: &mut Vec<UserError>,
) -> Option<String> {
    let id = text(input, "catalogId")?;
    let Some(catalog) = super::catalog_key(state, id) else {
        // The code's description in the schema.
        errors.push(coded(
            &["input", "catalogId"],
            "The specified catalog does not exist.",
            "CATALOG_DOES_NOT_EXIST",
        ));
        return None;
    };
    let record = state.get(&catalog)?;
    if record
        .get("priceList")
        .and_then(Value::as_str)
        .is_some_and(|other| Some(other) != list && state.contains(other))
    {
        // The code's description in the schema.
        errors.push(coded(
            &["input", "catalogId"],
            "The catalog is already associated with another price list.",
            "CATALOG_TAKEN",
        ));
        return None;
    }
    if strings(record.get("markets"))
        .iter()
        .any(|market| market_currency(state, market) != currency)
    {
        // The code's description in the schema.
        errors.push(coded(
            &["input", "currency"],
            "The currency specified does not match the catalog's market currency.",
            "CATALOG_MARKET_AND_PRICE_LIST_CURRENCY_MISMATCH",
        ));
        return None;
    }
    Some(catalog)
}

/// Whether a name is another price list's.
fn name_taken(state: &ShopState, name: &str, list: Option<&str>) -> bool {
    state.keys("PriceList").any(|other| {
        Some(other) != list
            && state
                .get(other)
                .and_then(|record| record.get("name"))
                .and_then(Value::as_str)
                .is_some_and(|known| known.eq_ignore_ascii_case(name))
    })
}

/// Links a price list to a catalog, unlinking the catalog it had.
fn link_catalog(state: &mut ShopState, list: &str, catalog: &str) {
    let previous = state
        .get(list)
        .and_then(|record| record.get("catalog"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(previous) = previous.filter(|previous| previous != catalog)
        && let Some(record) = state.get_mut(&previous)
    {
        record.insert("priceList".into(), Value::Null);
    }
    if let Some(record) = state.get_mut(catalog) {
        record.insert("priceList".into(), json!(list));
    }
    if let Some(record) = state.get_mut(list) {
        record.insert("catalog".into(), json!(catalog));
    }
}

/// `priceListCreate`.
fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = input(ctx, "input");
    let state = mutation.state();
    let mut errors = Vec::new();
    let name = text(&input, "name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        errors.push(coded(&["input", "name"], "Name can't be blank", "BLANK"));
    } else if name_taken(state, &name, None) {
        errors.push(coded(
            &["input", "name"],
            "Name has already been taken",
            "TAKEN",
        ));
    }
    let currency = text(&input, "currency").unwrap_or_default().to_string();
    let parent = input
        .get("parent")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    check_parent(&parent, &["input", "parent"], &mut errors);
    let catalog = check_catalog(state, &input, None, &currency, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"priceList": null}), &errors));
    }
    let state = mutation.state_mut();
    let list = state.new_gid("PriceList");
    let now = state.timestamp();
    state.insert(value::object(json!({
        "__typename": "PriceList",
        "id": list,
        "name": name,
        "currency": currency,
        "parent": parent_of(&parent),
        "catalog": null,
        "createdAt": now,
        "fixedPricesCount": 0,
        "lsfFixedPrices": [],
        "lsfQuantityRules": [],
        "lsfPriceBreaks": [],
    })));
    if let Some(catalog) = catalog {
        link_catalog(state, &list, &catalog);
    }
    state.emit("price_lists/create", &list);
    Ok(payload(json!({"priceList": list}), &[]))
}

/// `priceListUpdate`. A new currency removes the fixed prices (and their price breaks).
fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = input(ctx, "input");
    let state = mutation.state();
    let Some(list) = list_arg(state, ctx, "id") else {
        return Ok(payload(
            json!({"priceList": null}),
            &[list_not_found("id", "PRICE_LIST_NOT_FOUND")],
        ));
    };
    let record = state.get(&list).cloned().unwrap_or_default();
    let mut errors = Vec::new();
    let name = text(&input, "name").map(|name| name.trim().to_string());
    if let Some(name) = &name {
        if name.is_empty() {
            errors.push(coded(&["input", "name"], "Name can't be blank", "BLANK"));
        } else if name_taken(state, name, Some(&list)) {
            errors.push(coded(
                &["input", "name"],
                "Name has already been taken",
                "TAKEN",
            ));
        }
    }
    let old_currency = record
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let currency = text(&input, "currency").map_or(old_currency.clone(), str::to_string);
    let parent = input.get("parent").and_then(Value::as_object).cloned();
    if let Some(parent) = &parent {
        check_parent(parent, &["input", "parent"], &mut errors);
    }
    let catalog = check_catalog(state, &input, Some(&list), &currency, &mut errors);
    if catalog.is_none()
        && currency != old_currency
        && let Some(own) = record.get("catalog").and_then(Value::as_str)
        && strings(state.get(own).and_then(|catalog| catalog.get("markets")))
            .iter()
            .any(|market| market_currency(state, market) != currency)
    {
        // The code's description in the schema.
        errors.push(coded(
            &["input", "currency"],
            "The currency specified does not match the catalog's market currency.",
            "CATALOG_MARKET_AND_PRICE_LIST_CURRENCY_MISMATCH",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"priceList": null}), &errors));
    }
    let state = mutation.state_mut();
    if currency != old_currency {
        let variants: Vec<String> = fixed_variants(state, &list).into_iter().collect();
        remove_fixed(state, &list, &variants);
    }
    if let Some(record) = state.get_mut(&list) {
        if let Some(name) = name {
            record.insert("name".into(), json!(name));
        }
        record.insert("currency".into(), json!(currency));
        if let Some(parent) = &parent {
            record.insert("parent".into(), parent_of(parent));
        }
    }
    if let Some(catalog) = catalog {
        link_catalog(state, &list, &catalog);
    }
    state.emit("price_lists/update", &list);
    Ok(payload(json!({"priceList": list}), &[]))
}

/// `priceListDelete`.
fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let Some(list) = list_arg(mutation.state(), ctx, "id") else {
        return Ok(payload(
            json!({"deletedId": null}),
            &[list_not_found("id", "PRICE_LIST_NOT_FOUND")],
        ));
    };
    delete_price_list(mutation.state_mut(), &list);
    Ok(payload(json!({"deletedId": list}), &[]))
}

/// The currency of a price list.
fn currency_of(state: &ShopState, list: &str) -> String {
    state
        .get(list)
        .and_then(|record| record.get("currency"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `priceListFixedPricesAdd`.
fn fixed_prices_add(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(list) = list_arg(state, ctx, "priceListId") else {
        return Ok(payload(
            json!({"prices": []}),
            &[list_not_found("priceListId", "PRICE_LIST_NOT_FOUND")],
        ));
    };
    let currency = currency_of(state, &list);
    let mut errors = Vec::new();
    let prices = check_prices(
        state,
        &objects(ctx.arg("prices")),
        &["prices"],
        &currency,
        ("VARIANT_NOT_FOUND", "PRICE_LIST_CURRENCY_MISMATCH"),
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"prices": []}), &errors));
    }
    let state = mutation.state_mut();
    set_fixed(state, &list, &prices);
    state.emit("price_lists/update", &list);
    let variants: Vec<String> = prices.iter().map(|price| price.variant.clone()).collect();
    Ok(payload(
        json!({"prices": prices_of(mutation.state(), &list, &variants)}),
        &[],
    ))
}

/// Checks variant ids to delete the fixed prices of: they must be variants with one.
fn check_deletes(
    state: &ShopState,
    list: &str,
    ids: &[String],
    at: &str,
    errors: &mut Vec<UserError>,
) {
    let fixed = fixed_variants(state, list);
    for (index, id) in ids.iter().enumerate() {
        let index = index.to_string();
        if !is(state, id, "ProductVariant") {
            errors.push(coded(
                &[at, &index],
                "Variant not found.",
                "VARIANT_NOT_FOUND",
            ));
        } else if !fixed.contains(id) {
            // The code's description in the schema.
            errors.push(coded(
                &[at, &index],
                "Only fixed prices can be deleted.",
                "PRICE_NOT_FIXED",
            ));
        }
    }
}

/// `priceListFixedPricesUpdate`: deletes, then adds.
fn fixed_prices_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let empty = json!({"priceList": null, "pricesAdded": [], "deletedFixedPriceVariantIds": []});
    let Some(list) = list_arg(state, ctx, "priceListId") else {
        return Ok(payload(
            empty,
            &[list_not_found("priceListId", "PRICE_LIST_NOT_FOUND")],
        ));
    };
    let currency = currency_of(state, &list);
    let mut errors = Vec::new();
    let to_delete = strings(ctx.arg("variantIdsToDelete"));
    check_deletes(state, &list, &to_delete, "variantIdsToDelete", &mut errors);
    let prices = check_prices(
        state,
        &objects(ctx.arg("pricesToAdd")),
        &["pricesToAdd"],
        &currency,
        ("VARIANT_NOT_FOUND", "PRICE_LIST_CURRENCY_MISMATCH"),
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    remove_fixed(state, &list, &to_delete);
    set_fixed(state, &list, &prices);
    state.emit("price_lists/update", &list);
    let variants: Vec<String> = prices.iter().map(|price| price.variant.clone()).collect();
    let mut deleted = to_delete;
    deleted.sort_by_key(|variant| number(variant));
    Ok(payload(
        json!({
            "priceList": list,
            "pricesAdded": prices_of(mutation.state(), &list, &variants),
            "deletedFixedPriceVariantIds": deleted,
        }),
        &[],
    ))
}

/// `priceListFixedPricesDelete`.
fn fixed_prices_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(list) = list_arg(state, ctx, "priceListId") else {
        return Ok(payload(
            json!({"deletedFixedPriceVariantIds": null}),
            &[list_not_found("priceListId", "PRICE_LIST_NOT_FOUND")],
        ));
    };
    let ids = strings(ctx.arg("variantIds"));
    let mut errors = Vec::new();
    check_deletes(state, &list, &ids, "variantIds", &mut errors);
    if !errors.is_empty() {
        return Ok(payload(
            json!({"deletedFixedPriceVariantIds": null}),
            &errors,
        ));
    }
    let state = mutation.state_mut();
    remove_fixed(state, &list, &ids);
    state.emit("price_lists/update", &list);
    let mut deleted = ids;
    deleted.sort_by_key(|variant| number(variant));
    deleted.dedup();
    Ok(payload(
        json!({"deletedFixedPriceVariantIds": deleted}),
        &[],
    ))
}

/// `priceListFixedPricesByProductUpdate`: the same price for every variant of a product.
fn fixed_prices_by_product_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let empty =
        json!({"priceList": null, "pricesToAddProducts": null, "pricesToDeleteProducts": null});
    let Some(list) = list_arg(state, ctx, "priceListId") else {
        // The code's description in the schema.
        let error = coded(
            &["priceListId"],
            "Price list does not exist.",
            "PRICE_LIST_DOES_NOT_EXIST",
        );
        return Ok(payload(empty, &[error]));
    };
    let currency = currency_of(state, &list);
    let to_add = objects(ctx.arg("pricesToAdd"));
    let to_delete = strings(ctx.arg("pricesToDeleteByProductIds"));
    let mut errors = Vec::new();
    if to_add.is_empty() && to_delete.is_empty() {
        // The code's description in the schema.
        errors.push(coded(
            &[],
            "At least one update operation must be specified.",
            "NO_UPDATE_OPERATIONS_SPECIFIED",
        ));
    }
    let mut added_products: Vec<String> = Vec::new();
    let mut prices = Vec::new();
    for (index, input) in to_add.iter().enumerate() {
        let index = index.to_string();
        let product = text(input, "productId").unwrap_or_default().to_string();
        if !is(state, &product, "Product") {
            errors.push(coded(
                &["pricesToAdd", &index, "productId"],
                "Product does not exist.",
                "PRODUCT_DOES_NOT_EXIST",
            ));
            continue;
        }
        if added_products.contains(&product) {
            errors.push(coded(
                &["pricesToAdd", &index, "productId"],
                "Duplicate ID in input.",
                "DUPLICATE_ID_IN_INPUT",
            ));
            continue;
        }
        if to_delete.contains(&product) {
            errors.push(coded(
                &["pricesToAdd", &index, "productId"],
                "IDs must be mutually exclusive across add or delete operations.",
                "ID_MUST_BE_MUTUALLY_EXCLUSIVE",
            ));
            continue;
        }
        let price = input
            .get("price")
            .and_then(Value::as_object)
            .and_then(money);
        let compare = input
            .get("compareAtPrice")
            .and_then(Value::as_object)
            .and_then(money);
        if price.as_ref().is_some_and(|(_, given)| *given != currency)
            || compare
                .as_ref()
                .is_some_and(|(_, given)| *given != currency)
        {
            errors.push(coded(
                &["pricesToAdd", &index, "price", "currencyCode"],
                "Prices to add inputs must all have the same currency.",
                "PRICES_TO_ADD_CURRENCY_MISMATCH",
            ));
            continue;
        }
        added_products.push(product.clone());
        if let Some((cents, _)) = price {
            for variant in gids(
                state
                    .get(&product)
                    .and_then(|record| record.get("variants")),
            ) {
                prices.push(FixedPrice {
                    variant,
                    price: value::money_v2(cents, &currency),
                    compare_at: compare
                        .as_ref()
                        .map(|(cents, _)| value::money_v2(*cents, &currency))
                        .unwrap_or(Value::Null),
                });
            }
        }
    }
    let mut deleted_products = Vec::new();
    for (index, product) in to_delete.iter().enumerate() {
        if !is(state, product, "Product") {
            errors.push(coded(
                &["pricesToDeleteByProductIds", &index.to_string()],
                "Product does not exist.",
                "PRODUCT_DOES_NOT_EXIST",
            ));
        } else if !deleted_products.contains(product) {
            deleted_products.push(product.clone());
        }
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let variants_to_delete: Vec<String> = deleted_products
        .iter()
        .flat_map(|product| gids(state.get(product).and_then(|record| record.get("variants"))))
        .collect();
    let state = mutation.state_mut();
    remove_fixed(state, &list, &variants_to_delete);
    set_fixed(state, &list, &prices);
    state.emit("price_lists/update", &list);
    Ok(payload(
        json!({
            "priceList": list,
            "pricesToAddProducts": added_products,
            "pricesToDeleteProducts": deleted_products,
        }),
        &[],
    ))
}

// --- Quantity rules and price breaks ---

/// A quantity rule input, checked.
struct Rule {
    variant: String,
    minimum: i64,
    maximum: Option<i64>,
    increment: i64,
}

/// The codes of the checks of a rule, which the two mutations name differently:
/// `quantityRulesAdd`'s and `quantityPricingByVariantUpdate`'s.
struct RuleCodes {
    variant: &'static str,
    duplicate: &'static str,
    increment_over_minimum: &'static str,
    maximum_multiple: &'static str,
    minimum_multiple: &'static str,
    minimum_over_maximum: &'static str,
    below_one: &'static str,
    /// The field of each error: the input's field (`quantityRulesAdd`), or the input itself.
    by_field: bool,
}

const RULES_ADD: RuleCodes = RuleCodes {
    variant: "PRODUCT_VARIANT_DOES_NOT_EXIST",
    duplicate: "DUPLICATE_INPUT_FOR_VARIANT",
    increment_over_minimum: "INCREMENT_IS_GREATER_THAN_MINIMUM",
    maximum_multiple: "MAXIMUM_NOT_MULTIPLE_OF_INCREMENT",
    minimum_multiple: "MINIMUM_NOT_MULTIPLE_OF_INCREMENT",
    minimum_over_maximum: "MINIMUM_IS_GREATER_THAN_MAXIMUM",
    below_one: "GREATER_THAN_OR_EQUAL_TO",
    by_field: true,
};

const PRICING_RULES: RuleCodes = RuleCodes {
    variant: "QUANTITY_RULE_ADD_VARIANT_NOT_FOUND",
    duplicate: "QUANTITY_RULE_ADD_DUPLICATE_INPUT_FOR_VARIANT",
    increment_over_minimum: "QUANTITY_RULE_ADD_INCREMENT_IS_GREATER_THAN_MINIMUM",
    maximum_multiple: "QUANTITY_RULE_ADD_MAXIMUM_NOT_A_MULTIPLE_OF_INCREMENT",
    minimum_multiple: "QUANTITY_RULE_ADD_MINIMUM_NOT_A_MULTIPLE_OF_INCREMENT",
    minimum_over_maximum: "QUANTITY_RULE_ADD_MINIMUM_GREATER_THAN_MAXIMUM",
    below_one: "QUANTITY_RULE_ADD_MINIMUM_IS_LESS_THAN_ONE",
    by_field: false,
};

/// Checks quantity rule inputs at `at`, as the example of `quantityRulesAdd` answers them:
/// for each input, the increment, then the maximum, then the minimum.
fn check_rules(
    state: &ShopState,
    inputs: &[Map<String, Value>],
    at: &[&str],
    codes: &RuleCodes,
    errors: &mut Vec<UserError>,
) -> Vec<Rule> {
    let mut rules: Vec<Rule> = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let index = index.to_string();
        let path = |field: &str| -> Vec<String> {
            let mut path: Vec<String> = at.iter().map(|part| part.to_string()).collect();
            path.push(index.clone());
            if codes.by_field {
                path.push(field.to_string());
            }
            path
        };
        let error = |field: &str, message: &str, code: &str| UserError {
            field: Some(path(field)),
            message: message.to_string(),
            code: Some(code.to_string()),
        };
        let before = errors.len();
        let variant = text(input, "variantId").unwrap_or_default().to_string();
        let minimum = input.get("minimum").and_then(Value::as_i64).unwrap_or(1);
        let maximum = input.get("maximum").and_then(Value::as_i64);
        let increment = input.get("increment").and_then(Value::as_i64).unwrap_or(1);
        if !is(state, &variant, "ProductVariant") {
            // The code's description in the schema.
            errors.push(error(
                "variantId",
                "Product variant ID does not exist.",
                codes.variant,
            ));
        } else if rules.iter().any(|rule| rule.variant == variant) {
            errors.push(error(
                "variantId",
                "Quantity rule inputs must be unique by variant id.",
                codes.duplicate,
            ));
        }
        for (field, value) in [
            ("minimum", Some(minimum)),
            ("maximum", maximum),
            ("increment", Some(increment)),
        ] {
            if value.is_some_and(|value| value < 1) {
                // lsf's wording, Rails' numericality.
                let name = format!("{}{}", field[..1].to_uppercase(), &field[1..]);
                errors.push(error(
                    field,
                    &format!("{name} must be greater than or equal to 1"),
                    codes.below_one,
                ));
            }
        }
        if errors.len() == before {
            // The messages of the reference's example.
            if increment > minimum {
                errors.push(error(
                    "increment",
                    "Increment must be lower than or equal to the minimum.",
                    codes.increment_over_minimum,
                ));
            }
            if maximum.is_some_and(|maximum| maximum % increment != 0) {
                errors.push(error(
                    "maximum",
                    "Maximum must be a multiple of the increment.",
                    codes.maximum_multiple,
                ));
            }
            if minimum % increment != 0 {
                errors.push(error(
                    "minimum",
                    "Minimum must be a multiple of the increment.",
                    codes.minimum_multiple,
                ));
            }
            if maximum.is_some_and(|maximum| minimum > maximum) {
                errors.push(error(
                    "minimum",
                    "Minimum must be lower than or equal to the maximum.",
                    codes.minimum_over_maximum,
                ));
            }
        }
        if errors.len() == before {
            rules.push(Rule {
                variant,
                minimum,
                maximum,
                increment,
            });
        }
    }
    rules
}

/// Sets quantity rules on a price list, replacing those of the same variants.
fn set_rules(state: &mut ShopState, list: &str, rules: &[Rule]) {
    if let Some(record) = state.get_mut(list) {
        let entry = record
            .entry("lsfQuantityRules")
            .or_insert_with(|| json!([]));
        if let Value::Array(items) = entry {
            for rule in rules {
                items.retain(|known| known.get("variant") != Some(&json!(rule.variant)));
                items.push(json!({
                    "variant": rule.variant,
                    "minimum": rule.minimum,
                    "maximum": rule.maximum,
                    "increment": rule.increment,
                }));
            }
        }
    }
}

/// Removes the quantity rules of variants.
fn remove_rules(state: &mut ShopState, list: &str, variants: &[String]) {
    if let Some(Value::Array(items)) = state
        .get_mut(list)
        .and_then(|record| record.get_mut("lsfQuantityRules"))
    {
        items.retain(|known| {
            !known
                .get("variant")
                .and_then(Value::as_str)
                .is_some_and(|variant| variants.iter().any(|gone| gone == variant))
        });
    }
}

/// The variants with a quantity rule on a price list.
fn ruled_variants(state: &ShopState, list: &str) -> HashSet<String> {
    state
        .get(list)
        .and_then(|record| record.get("lsfQuantityRules"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|rule| rule.get("variant")?.as_str().map(str::to_string))
        .collect()
}

/// The minimum quantities of a variant's price breaks on a price list.
fn break_minimums(state: &ShopState, list: &str, variant: &str) -> Vec<i64> {
    breaks_of(state, list, variant)
        .iter()
        .filter_map(|price_break| state.get(price_break)?.get("minimumQuantity")?.as_i64())
        .collect()
}

/// Checks rules against the price breaks a variant has (`quantityRulesAdd`).
fn check_rules_against_breaks(
    state: &ShopState,
    list: &str,
    rules: &[Rule],
    inputs_at: &[&str],
    inputs: &[Map<String, Value>],
    errors: &mut Vec<UserError>,
) {
    for rule in rules {
        let Some(index) = inputs
            .iter()
            .position(|input| text(input, "variantId") == Some(rule.variant.as_str()))
        else {
            continue;
        };
        let index = index.to_string();
        let mut field: Vec<&str> = inputs_at.to_vec();
        field.push(&index);
        for minimum in break_minimums(state, list, &rule.variant) {
            // The codes' descriptions in the schema.
            if rule.minimum > minimum {
                let mut at = field.clone();
                at.push("minimum");
                errors.push(coded(
                    &at,
                    "Minimum must be lower than or equal to the minimum quantity of the variant's quantity price breaks.",
                    "MINIMUM_IS_HIGHER_THAN_QUANTITY_PRICE_BREAK_MINIMUM",
                ));
            }
            if rule.maximum.is_some_and(|maximum| maximum < minimum) {
                let mut at = field.clone();
                at.push("maximum");
                errors.push(coded(
                    &at,
                    "Maximum must be greater than or equal to the minimum quantity of the variant's quantity price breaks.",
                    "MAXIMUM_IS_LOWER_THAN_QUANTITY_PRICE_BREAK_MINIMUM",
                ));
            }
            if minimum % rule.increment != 0 {
                let mut at = field.clone();
                at.push("increment");
                errors.push(coded(
                    &at,
                    "Increment must be a multiple of the minimum quantity of the variant's quantity price breaks.",
                    "INCREMENT_NOT_A_MULTIPLE_OF_QUANTITY_PRICE_BREAK_MINIMUM",
                ));
            }
        }
    }
}

/// `quantityRulesAdd`.
fn quantity_rules_add(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(list) = list_arg(state, ctx, "priceListId") else {
        let error = coded(
            &["priceListId"],
            "Price list does not exist.",
            "PRICE_LIST_DOES_NOT_EXIST",
        );
        return Ok(payload(json!({"quantityRules": []}), &[error]));
    };
    if !supports_quantity(state, &list) {
        let error = coded(
            &["priceListId"],
            "Quantity rules can be associated only with company location catalogs.",
            "CATALOG_CONTEXT_DOES_NOT_SUPPORT_QUANTITY_RULES",
        );
        return Ok(payload(json!({"quantityRules": []}), &[error]));
    }
    let inputs = objects(ctx.arg("quantityRules"));
    let mut errors = Vec::new();
    let rules = check_rules(state, &inputs, &["quantityRules"], &RULES_ADD, &mut errors);
    check_rules_against_breaks(
        state,
        &list,
        &rules,
        &["quantityRules"],
        &inputs,
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"quantityRules": []}), &errors));
    }
    let state = mutation.state_mut();
    set_rules(state, &list, &rules);
    state.emit("price_lists/update", &list);
    let answered: Vec<Value> = rules
        .iter()
        .map(|rule| {
            rule_object(
                &rule.variant,
                Some(&json!({"minimum": rule.minimum, "maximum": rule.maximum, "increment": rule.increment})),
            )
        })
        .collect();
    Ok(payload(json!({"quantityRules": answered}), &[]))
}

/// `quantityRulesDelete`.
fn quantity_rules_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(list) = list_arg(state, ctx, "priceListId") else {
        let error = coded(
            &["priceListId"],
            "Price list does not exist.",
            "PRICE_LIST_DOES_NOT_EXIST",
        );
        return Ok(payload(
            json!({"deletedQuantityRulesVariantIds": null}),
            &[error],
        ));
    };
    let ids = strings(ctx.arg("variantIds"));
    let ruled = ruled_variants(state, &list);
    let mut errors = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let index = index.to_string();
        if !is(state, id, "ProductVariant") {
            errors.push(coded(
                &["variantIds", &index],
                "Product variant ID does not exist.",
                "PRODUCT_VARIANT_DOES_NOT_EXIST",
            ));
        } else if !ruled.contains(id) {
            // The code's description in the schema.
            errors.push(coded(
                &["variantIds", &index],
                "Quantity rule for variant associated with the price list provided does not exist.",
                "VARIANT_QUANTITY_RULE_DOES_NOT_EXIST",
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(
            json!({"deletedQuantityRulesVariantIds": null}),
            &errors,
        ));
    }
    let state = mutation.state_mut();
    remove_rules(state, &list, &ids);
    state.emit("price_lists/update", &list);
    Ok(payload(json!({"deletedQuantityRulesVariantIds": ids}), &[]))
}

/// Adds a price break record to a price list.
fn add_break(
    state: &mut ShopState,
    list: &str,
    variant: &str,
    minimum: i64,
    price: Value,
) -> String {
    let price_break = state.new_gid("QuantityPriceBreak");
    state.insert(value::object(json!({
        "__typename": "QuantityPriceBreak",
        "id": price_break,
        "minimumQuantity": minimum,
        "price": price,
        "priceList": list,
        "variant": variant,
    })));
    if let Some(record) = state.get_mut(list) {
        let entry = record.entry("lsfPriceBreaks").or_insert_with(|| json!([]));
        if let Value::Array(items) = entry {
            items.push(json!(price_break));
        }
    }
    price_break
}

/// Removes a price break record from its price list.
fn remove_break(state: &mut ShopState, list: &str, price_break: &str) {
    if let Some(Value::Array(items)) = state
        .get_mut(list)
        .and_then(|record| record.get_mut("lsfPriceBreaks"))
    {
        items.retain(|known| known.as_str() != Some(price_break));
    }
    state.remove(price_break);
}

/// `quantityPricingByVariantUpdate`: deletes (fixed prices, rules, price breaks), then adds
/// (fixed prices, rules, price breaks), all or nothing.
fn quantity_pricing_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(list) = list_arg(state, ctx, "priceListId") else {
        let error = coded(
            &["priceListId"],
            "Price list not found.",
            "PRICE_LIST_NOT_FOUND",
        );
        return Ok(payload(json!({"productVariants": null}), &[error]));
    };
    let input = input(ctx, "input");
    let list_of = |name: &str| objects(input.get(name));
    let ids_of = |name: &str| strings(input.get(name));
    let currency = currency_of(state, &list);
    let mut errors = Vec::new();
    let prices_to_delete = ids_of("pricesToDeleteByVariantId");
    let fixed = fixed_variants(state, &list);
    for (index, id) in prices_to_delete.iter().enumerate() {
        let at = ["input", "pricesToDeleteByVariantId", &index.to_string()].map(str::to_string);
        let at: Vec<&str> = at.iter().map(String::as_str).collect();
        if !is(state, id, "ProductVariant") {
            errors.push(coded(
                &at,
                "Price list price was not found for the variant.",
                "PRICE_DELETE_VARIANT_NOT_FOUND",
            ));
        } else if !fixed.contains(id) {
            errors.push(coded(
                &at,
                "Only fixed prices can be deleted.",
                "PRICE_DELETE_PRICE_NOT_FIXED",
            ));
        }
    }
    let rules_to_delete = ids_of("quantityRulesToDeleteByVariantId");
    let ruled = ruled_variants(state, &list);
    for (index, id) in rules_to_delete.iter().enumerate() {
        let at = [
            "input",
            "quantityRulesToDeleteByVariantId",
            &index.to_string(),
        ]
        .map(str::to_string);
        let at: Vec<&str> = at.iter().map(String::as_str).collect();
        if !is(state, id, "ProductVariant") {
            errors.push(coded(
                &at,
                "Quantity rule was not found for the variant.",
                "QUANTITY_RULE_DELETE_VARIANT_NOT_FOUND",
            ));
        } else if !ruled.contains(id) {
            errors.push(coded(
                &at,
                "Quantity rule was not found for the variant.",
                "QUANTITY_RULE_DELETE_RULE_NOT_FOUND",
            ));
        }
    }
    let breaks_to_delete = ids_of("quantityPriceBreaksToDelete");
    let own_breaks = gids(
        state
            .get(&list)
            .and_then(|record| record.get("lsfPriceBreaks")),
    );
    for (index, id) in breaks_to_delete.iter().enumerate() {
        if !own_breaks.contains(id) {
            errors.push(coded(
                &["input", "quantityPriceBreaksToDelete", &index.to_string()],
                "Quantity price break was not found.",
                "QUANTITY_PRICE_BREAK_DELETE_NOT_FOUND",
            ));
        }
    }
    let breaks_by_variant = ids_of("quantityPriceBreaksToDeleteByVariantId");
    for (index, id) in breaks_by_variant.iter().enumerate() {
        if !is(state, id, "ProductVariant") {
            errors.push(coded(
                &[
                    "input",
                    "quantityPriceBreaksToDeleteByVariantId",
                    &index.to_string(),
                ],
                "Variant was not found.",
                "QUANTITY_PRICE_BREAK_DELETE_BY_VARIANT_ID_VARIANT_NOT_FOUND",
            ));
        }
    }
    let prices = check_prices(
        state,
        &list_of("pricesToAdd"),
        &["input", "pricesToAdd"],
        &currency,
        ("PRICE_ADD_VARIANT_NOT_FOUND", "PRICE_ADD_CURRENCY_MISMATCH"),
        &mut errors,
    );
    let mut seen = HashSet::new();
    for (index, price) in list_of("pricesToAdd").iter().enumerate() {
        if let Some(variant) = text(price, "variantId")
            && !seen.insert(variant.to_string())
        {
            errors.push(coded(
                &["input", "pricesToAdd", &index.to_string()],
                "Prices to add inputs must be unique by variant id.",
                "PRICE_ADD_DUPLICATE_INPUT_FOR_VARIANT",
            ));
        }
    }
    let rule_inputs = list_of("quantityRulesToAdd");
    let rules = check_rules(
        state,
        &rule_inputs,
        &["input", "quantityRulesToAdd"],
        &PRICING_RULES,
        &mut errors,
    );
    let break_inputs = list_of("quantityPriceBreaksToAdd");
    if (!rule_inputs.is_empty() || !break_inputs.is_empty()) && !supports_quantity(state, &list) {
        errors.push(coded(
            &["input", "quantityRulesToAdd"],
            "Quantity rules can be associated only with company location catalogs.",
            "QUANTITY_RULE_ADD_CATALOG_CONTEXT_NOT_SUPPORTED",
        ));
    }
    // What the price list will have once the deletes and the fixed prices are applied.
    let mut fixed_after: HashSet<String> = fixed.clone();
    for id in &prices_to_delete {
        fixed_after.remove(id);
    }
    for price in &prices {
        fixed_after.insert(price.variant.clone());
    }
    let rule_after = |variant: &str| -> Option<(i64, Option<i64>, i64)> {
        if let Some(rule) = rules.iter().find(|rule| rule.variant == variant) {
            return Some((rule.minimum, rule.maximum, rule.increment));
        }
        if rules_to_delete.iter().any(|gone| gone == variant) {
            return None;
        }
        let rule = state
            .get(&list)
            .and_then(|record| rule_of(record, variant))?;
        Some((
            rule.get("minimum")?.as_i64()?,
            rule.get("maximum").and_then(Value::as_i64),
            rule.get("increment").and_then(Value::as_i64).unwrap_or(1),
        ))
    };
    let mut breaks = Vec::new();
    let mut break_keys = HashSet::new();
    for (index, input) in break_inputs.iter().enumerate() {
        let index = index.to_string();
        let at = ["input", "quantityPriceBreaksToAdd", index.as_str()];
        let variant = text(input, "variantId").unwrap_or_default().to_string();
        let minimum = input
            .get("minimumQuantity")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let before = errors.len();
        if !is(state, &variant, "ProductVariant") {
            errors.push(coded(
                &at,
                "Variant was not found.",
                "QUANTITY_PRICE_BREAK_ADD_VARIANT_NOT_FOUND",
            ));
            continue;
        }
        if minimum < 2 {
            // lsf's wording: a break applies from a quantity above the first unit.
            errors.push(coded(
                &at,
                "Minimum quantity must be greater than 1.",
                "QUANTITY_PRICE_BREAK_ADD_INVALID",
            ));
        }
        if !break_keys.insert((variant.clone(), minimum)) {
            errors.push(coded(
                &at,
                "Quantity price break inputs must be unique by variant id and minimum quantity.",
                "QUANTITY_PRICE_BREAK_ADD_DUPLICATE_INPUT_FOR_VARIANT_AND_MIN",
            ));
        }
        let price = input
            .get("price")
            .and_then(Value::as_object)
            .and_then(money);
        if price.as_ref().is_some_and(|(_, given)| *given != currency) {
            errors.push(coded(
                &at,
                "The specified currency does not match the price list's currency.",
                "QUANTITY_PRICE_BREAK_ADD_CURRENCY_MISMATCH",
            ));
        }
        if !fixed_after.contains(&variant) {
            errors.push(coded(
                &at,
                "Quantity price breaks can be added only to variants with a fixed price on the price list.",
                "QUANTITY_PRICE_BREAK_ADD_PRICE_LIST_PRICE_NOT_FOUND",
            ));
        }
        if let Some((rule_minimum, rule_maximum, increment)) = rule_after(&variant) {
            if minimum < rule_minimum {
                // The messages of the reference's example, its typo included.
                errors.push(coded(
                    &at,
                    "Miniumum quantity price break must be greater than or equal to the minimum quantity rule.",
                    "QUANTITY_PRICE_BREAK_ADD_MIN_LOWER_THAN_QUANTITY_RULES_MIN",
                ));
                if let Some(rule_index) = rule_inputs
                    .iter()
                    .position(|rule| text(rule, "variantId") == Some(variant.as_str()))
                {
                    errors.push(coded(
                        &["input", "quantityRulesToAdd", &rule_index.to_string()],
                        "Minimum quantity rule must be less than or equal to the minimum quantity price break.",
                        "QUANTITY_RULE_ADD_MIN_HIGHER_THAN_QUANTITY_PRICE_BREAK_MIN",
                    ));
                }
            }
            if rule_maximum.is_some_and(|maximum| minimum > maximum) {
                errors.push(coded(
                    &at,
                    "Minimum quantity price break must be lower than or equal to the maximum quantity rule.",
                    "QUANTITY_PRICE_BREAK_ADD_MIN_HIGHER_THAN_QUANTITY_RULES_MAX",
                ));
            }
            if minimum % increment != 0 {
                errors.push(coded(
                    &at,
                    "Minimum quantity price break must be a multiple of the quantity rule's increment.",
                    "QUANTITY_PRICE_BREAK_ADD_MIN_NOT_A_MULTIPLE_OF_QUANTITY_RULES_INCREMENT",
                ));
            }
        }
        if errors.len() == before
            && let Some((cents, _)) = price
        {
            breaks.push((variant, minimum, value::money_v2(cents, &currency)));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"productVariants": null}), &errors));
    }
    let mut touched: Vec<String> = Vec::new();
    let mut touch = |variant: &str| {
        if !touched.iter().any(|known| known == variant) {
            touched.push(variant.to_string());
        }
    };
    for variant in prices_to_delete
        .iter()
        .chain(&rules_to_delete)
        .chain(&breaks_by_variant)
    {
        touch(variant);
    }
    for price in &prices {
        touch(&price.variant);
    }
    for rule in &rules {
        touch(&rule.variant);
    }
    for (variant, _, _) in &breaks {
        touch(variant);
    }
    let state = mutation.state_mut();
    for price_break in &breaks_to_delete {
        if let Some(variant) = state
            .get(price_break)
            .and_then(|record| record.get("variant"))
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            touch(&variant);
        }
    }
    remove_fixed(state, &list, &prices_to_delete);
    remove_rules(state, &list, &rules_to_delete);
    for price_break in &breaks_to_delete {
        remove_break(state, &list, price_break);
    }
    for variant in &breaks_by_variant {
        for price_break in breaks_of(state, &list, variant) {
            remove_break(state, &list, &price_break);
        }
    }
    set_fixed(state, &list, &prices);
    set_rules(state, &list, &rules);
    for (variant, minimum, price) in breaks {
        // A break of the same variant and minimum is replaced.
        for existing in breaks_of(state, &list, &variant) {
            if state
                .get(&existing)
                .and_then(|record| record.get("minimumQuantity"))
                .and_then(Value::as_i64)
                == Some(minimum)
            {
                remove_break(state, &list, &existing);
            }
        }
        add_break(state, &list, &variant, minimum, price);
    }
    state.emit("price_lists/update", &list);
    Ok(payload(json!({"productVariants": touched}), &[]))
}
