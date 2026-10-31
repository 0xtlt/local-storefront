//! The markets family's mutations and queries, over the demo store: what each changes and
//! answers, the events it emits, and that a refused input changes nothing.

use serde_json::{Value, json};

use crate::graphql::{self, Request};
use crate::resolve::{Registry, ShopResolver};
use crate::state::{Clock, ShopState};

fn demo() -> ShopState {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let mut state = ShopState::seeded(&store, None);
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-31T12:00:00Z").unwrap();
    state.set_clock(Clock::Frozen(now.with_timezone(&chrono::Utc)));
    state
}

/// The answer to a query with variables, `extensions` without `cost`.
fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let mut answer = graphql::run(
        crate::schema::admin(),
        &mut resolver,
        &Request::new(query).with_variables(variables),
        None,
    );
    answer.extensions.extend(resolver.extensions());
    answer.extensions.remove("cost");
    answer.to_json()
}

#[test]
fn scratch() {
    let mut shop = demo();
    let answer = run(
        &mut shop,
        r#"{
      markets(first: 5) { nodes { id name handle type status enabled primary
        conditions { conditionTypes regionsCondition { applicationLevel regions(first: 5) { nodes { id name ... on MarketRegionCountry { code currency { currencyCode currencyName enabled manualRate rateUpdatedAt } } } } } companyLocationsCondition { applicationLevel } }
        currencySettings { baseCurrency { currencyCode currencyName enabled manualRate rateUpdatedAt } localCurrencies roundingEnabled }
        priceInclusions { adaptivePricingEnabled inclusiveDutiesPricingStrategy inclusiveTaxPricingStrategy }
        webPresences(first: 2) { nodes { id } } catalogs(first: 2) { nodes { id } } catalogsCount { count }
        webPresence { id } priceList { id } returnPolicyProfile { id } delivery { shipping { isEnabled } }
      } }
      webPresences(first: 2) { nodes { id subfolderSuffix domain { id host localization { defaultLocale alternateLocales country } marketWebPresence { id } } defaultLocale { locale } alternateLocales { locale } rootUrls { locale url } markets(first: 5) { nodes { name } } market { name } } }
      shopLocales { locale name primary published marketWebPresences { id } }
      backupRegion { id name ... on MarketRegionCountry { code } }
      catalogs(first: 5) { nodes { id title status __typename publication { id name autoPublish supportsFuturePublishing catalog { id } } priceList { id } } }
      publications(first: 5) { nodes { id name autoPublish supportsFuturePublishing catalog { id title } } }
      publicationsCount { count } catalogsCount { count }
      marketByGeography(countryCode: FR) { name }
      marketsResolvedValues(buyerSignal: {countryCode: FR}) { currencyCode priceInclusivity { dutiesIncluded taxesIncluded } catalogs(first: 1) { nodes { id } } webPresences(first: 1) { nodes { id } } }
      availableLocales { isoCode name }
      products(first: 1) { nodes { contextualPricing(context: {country: FR}) { priceRange { minVariantPrice { amount currencyCode } } minVariantPricing { price { amount } quantityRule { minimum increment maximum isDefault originType } } fixedQuantityRulesCount } variants(first: 1) { nodes { contextualPricing(context: {country: FR}) { price { amount currencyCode } compareAtPrice { amount } quantityRule { minimum } quantityPriceBreaks(first: 2) { nodes { id } } } } } } }
      channels(first: 2) { nodes { id } }
      priceLists(first: 2) { nodes { id } }
    }"#,
        json!({}),
    );
    let mut short = answer.clone();
    short["data"]["availableLocales"] = json!(
        answer["data"]["availableLocales"]
            .as_array()
            .map(|a| a.len())
    );
    println!("{}", serde_json::to_string_pretty(&short).unwrap());
}
