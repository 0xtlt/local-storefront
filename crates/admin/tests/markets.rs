//! The markets family from the outside: the records its mutations make checked against the
//! schema, and what they change projected into the store the storefront sees (a market's
//! countries and currency, the shop's published languages), and what stays in the Admin API
//! (catalogs, price lists, channels).

mod support;

use lsf_admin::graphql::{self, Request};
use lsf_admin::resolve::{Registry, ShopResolver};
use lsf_admin::state::{Clock, ShopState};
use lsf_admin::sync;
use lsf_core::store::load::{DataSource, LoadOptions, load_with_overlays};
use serde_json::{Value, json};
use support::shape::Schema;

fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let answer = graphql::run(
        lsf_admin::schema::admin(),
        &mut resolver,
        &Request::new(query).with_variables(variables),
        None,
    );
    answer.to_json()
}

/// Runs a mutation that must succeed: its payload.
fn ok(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let answer = run(shop, query, variables);
    assert!(answer.get("errors").is_none(), "{answer}");
    let payload = answer["data"]
        .as_object()
        .and_then(|data| data.values().next())
        .cloned()
        .unwrap_or_default();
    assert_eq!(payload["userErrors"], json!([]), "{answer}");
    payload
}

/// A shop of the rich fixture, on a frozen clock.
fn rich() -> (lsf_core::Store, ShopState) {
    let store = support::fixture_store("rich");
    let app = support::fixture_app();
    let mut shop = ShopState::seeded(&store, Some(&app));
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-31T12:00:00Z").unwrap();
    shop.set_clock(Clock::Frozen(now.with_timezone(&chrono::Utc)));
    (store, shop)
}

/// Every mutation of the family, once: markets, a web presence, a locale, catalogs, price lists
/// with fixed prices, rules and price breaks, a publication, a channel and its listings,
/// feedback, market localizations.
fn exercise(shop: &mut ShopState) {
    let europe = shop.keys("Market").next().unwrap().to_string();
    let location = shop.keys("CompanyLocation").next().unwrap().to_string();
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let product = shop.get(&variant).unwrap()["product"]
        .as_str()
        .unwrap()
        .to_string();
    // A market of Japan, in its own currency, and the European market in Swiss francs.
    let japan = ok(
        shop,
        "mutation { marketCreate(input: {name: \"Japan\", conditions: {regionsCondition: {regions: [{countryCode: JP}]}}}) { market { id } userErrors { message } } }",
        json!({}),
    )["market"]["id"]
        .clone();
    ok(
        shop,
        "mutation($id: ID!) { marketUpdate(id: $id, input: {handle: \"nihon\", currencySettings: {baseCurrency: JPY, localCurrencies: false}}) { market { id } userErrors { message } } }",
        json!({"id": japan}),
    );
    ok(
        shop,
        "mutation($id: ID!) { marketUpdate(id: $id, input: {currencySettings: {baseCurrency: CHF, localCurrencies: false}}) { market { id } userErrors { message } } }",
        json!({"id": europe}),
    );
    ok(
        shop,
        "mutation { webPresenceCreate(input: {defaultLocale: \"fr\", subfolderSuffix: \"ch\"}) { webPresence { id } userErrors { message } } }",
        json!({}),
    );
    ok(
        shop,
        "mutation { shopLocaleEnable(locale: \"de\") { shopLocale { locale } userErrors { message } } }",
        json!({}),
    );
    ok(
        shop,
        "mutation { shopLocaleUpdate(locale: \"de\", shopLocale: {published: true}) { shopLocale { locale } userErrors { message } } }",
        json!({}),
    );
    ok(
        shop,
        "mutation { backupRegionUpdate(region: {countryCode: FR}) { backupRegion { id } userErrors { message } } }",
        json!({}),
    );
    // A B2B catalog with a price list, its fixed price, rule and price break.
    let list = ok(
        shop,
        "mutation { priceListCreate(input: {name: \"Wholesale\", currency: EUR, parent: {adjustment: {type: PERCENTAGE_DECREASE, value: 20}}}) { priceList { id } userErrors { message } } }",
        json!({}),
    )["priceList"]["id"]
        .clone();
    ok(
        shop,
        "mutation($location: ID!, $list: ID!) { catalogCreate(input: {title: \"Wholesale\", status: ACTIVE, context: {companyLocationIds: [$location]}, priceListId: $list}) { catalog { id } userErrors { message } } }",
        json!({"location": location, "list": list}),
    );
    ok(
        shop,
        "mutation($list: ID!, $variant: ID!) { quantityPricingByVariantUpdate(priceListId: $list, input: {
            pricesToAdd: [{variantId: $variant, price: {amount: \"40\", currencyCode: EUR}}], pricesToDeleteByVariantId: [],
            quantityRulesToAdd: [{variantId: $variant, minimum: 10, maximum: 100, increment: 5}], quantityRulesToDeleteByVariantId: [],
            quantityPriceBreaksToAdd: [{variantId: $variant, minimumQuantity: 20, price: {amount: \"35\", currencyCode: EUR}}], quantityPriceBreaksToDelete: []
        }) { productVariants { id } userErrors { message } } }",
        json!({"list": list, "variant": variant}),
    );
    // A market catalog with a publication.
    let catalog = ok(
        shop,
        "mutation($market: ID!) { catalogCreate(input: {title: \"Europe\", status: ACTIVE, context: {marketIds: [$market]}}) { catalog { id } userErrors { message } } }",
        json!({"market": europe}),
    )["catalog"]["id"]
        .clone();
    ok(
        shop,
        "mutation($catalog: ID!) { publicationCreate(input: {catalogId: $catalog, autoPublish: true}) { publication { id } userErrors { message } } }",
        json!({"catalog": catalog}),
    );
    // A channel, a product listed on it, feedback.
    ok(
        shop,
        "mutation { channelCreate(input: {accountId: \"A1\", accountName: \"Marketplace\", specificationHandle: \"marketplace-eu\"}) { channel { id } userErrors { message } } }",
        json!({}),
    );
    ok(
        shop,
        "mutation($id: ID!) { publishablePublishToCurrentChannel(id: $id) { userErrors { message } } }",
        json!({"id": product}),
    );
    ok(
        shop,
        "mutation($id: ID!) { bulkProductResourceFeedbackCreate(feedbackInput: [{productId: $id, state: ACCEPTED, feedbackGeneratedAt: \"2026-10-30T00:00:00Z\", productUpdatedAt: \"2026-10-29T00:00:00Z\"}]) { userErrors { message } } }",
        json!({"id": product}),
    );
}

#[test]
fn the_records_the_markets_mutations_make_are_in_the_shape_of_their_types() {
    let Some(schema) = Schema::load() else {
        return;
    };
    let (_, mut shop) = rich();
    exercise(&mut shop);
    let problems = schema.check(&shop);
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn a_market_s_countries_and_currency_and_the_published_languages_reach_the_store() {
    let (base, mut shop) = rich();
    exercise(&mut shop);
    let overlay = sync::overlay(&shop, &base).expect("something to sync");
    let (store, diagnostics) = load_with_overlays(
        &DataSource::Directory(support::fixture("rich")),
        &LoadOptions::default(),
        std::slice::from_ref(&overlay),
    );
    let own: Vec<_> = diagnostics
        .items
        .iter()
        .filter(|item| item.file == sync::OVERLAY_NAME)
        .collect();
    assert!(own.is_empty(), "{own:?}\n{overlay:#?}");
    // Japan, created and renamed, sells in yen; the European market's countries in francs.
    let japan = store.market_of("JP").expect("the created market");
    assert_eq!(japan.handle, "nihon");
    assert_eq!(japan.countries, ["JP"]);
    assert_eq!(store.country("JP").unwrap().currency, "JPY");
    for code in ["FR", "DE"] {
        assert_eq!(store.country(code).unwrap().currency, "CHF", "{code}");
    }
    // Great Britain keeps its own pounds.
    assert_eq!(store.country("GB").unwrap().currency, "GBP");
    // German is published: served under `/de/`.
    let languages: Vec<&str> = store
        .languages
        .iter()
        .map(|language| language.iso_code.as_str())
        .collect();
    assert_eq!(languages, ["fr", "en", "de"]);
    // Catalogs, price lists, channels: not store data.
    let products: Vec<i64> = store
        .products
        .iter()
        .flat_map(|product| product.variants.iter().map(|variant| variant.price))
        .collect();
    let base_prices: Vec<i64> = base
        .products
        .iter()
        .flat_map(|product| product.variants.iter().map(|variant| variant.price))
        .collect();
    assert_eq!(
        products, base_prices,
        "price lists do not change the store's prices"
    );
}

#[test]
fn a_locale_disabled_leaves_the_store() {
    let (base, mut shop) = rich();
    ok(
        &mut shop,
        "mutation { shopLocaleDisable(locale: \"en\") { locale userErrors { message } } }",
        json!({}),
    );
    let overlay = sync::overlay(&shop, &base).expect("something to sync");
    let (store, _) = load_with_overlays(
        &DataSource::Directory(support::fixture("rich")),
        &LoadOptions::default(),
        std::slice::from_ref(&overlay),
    );
    let languages: Vec<&str> = store
        .languages
        .iter()
        .map(|language| language.iso_code.as_str())
        .collect();
    assert_eq!(languages, ["fr"]);
}
