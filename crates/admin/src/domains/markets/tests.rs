//! The markets family's mutations and queries, over the demo store (US, Canada and France, the
//! shop in Canada, English and French): what each mutation changes and answers, the events it
//! emits, and that a refused input changes nothing.

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
    // The demo store runs on the system clock: a frozen one gives the same dates every run.
    state.set_clock(Clock::Frozen(at("2026-10-31T12:00:00Z")));
    state
}

fn at(date: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(date)
        .unwrap()
        .with_timezone(&chrono::Utc)
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

/// The payload of a mutation that succeeded: no GraphQL error, no user error.
fn ok<'a>(answer: &'a Value, name: &str) -> &'a Value {
    assert!(answer.get("errors").is_none(), "{name}: {answer}");
    let payload = &answer["data"][name];
    assert_eq!(payload["userErrors"], json!([]), "{name}: {answer}");
    payload
}

/// Runs a mutation that must be refused: it answers user errors, and the shop is as it was
/// (records, counters, events). The user errors.
fn refused(shop: &mut ShopState, query: &str, variables: Value, name: &str) -> Value {
    shop.take_events();
    let before = shop.picture();
    let answer = run(shop, query, variables);
    assert!(answer.get("errors").is_none(), "{name}: {answer}");
    let errors = answer["data"][name]["userErrors"].clone();
    assert!(
        errors.as_array().is_some_and(|errors| !errors.is_empty()),
        "{name} was not refused: {answer}"
    );
    assert_eq!(shop.picture(), before, "{name} changed the shop");
    errors
}

/// The topics emitted since the last call, with the key of their record.
fn events(shop: &mut ShopState) -> Vec<(String, String)> {
    shop.take_events()
        .into_iter()
        .map(|event| (event.topic, event.gid))
        .collect()
}

fn topics(shop: &mut ShopState) -> Vec<String> {
    events(shop).into_iter().map(|(topic, _)| topic).collect()
}

/// The GID of the market of a country.
fn market_of(shop: &ShopState, code: &str) -> String {
    super::market_of_country(shop, code).unwrap()
}

/// The first variant of the first product.
fn first_variant(shop: &ShopState) -> (String, String) {
    let product = shop.keys("Product").next().unwrap().to_string();
    let variant =
        crate::domains::catalog::gids(shop.get(&product).unwrap().get("variants")).remove(0);
    (product, variant)
}

/// The first company location of the rich fixture's shop.
fn rich() -> ShopState {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rich");
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Directory(path),
        &Default::default(),
    );
    let mut state = ShopState::seeded(&store, None);
    state.set_clock(Clock::Frozen(at("2026-10-31T12:00:00Z")));
    state
}

const MARKET_FIELDS: &str = "id name handle type status enabled primary
    conditions { conditionTypes regionsCondition { regions(first: 10) { nodes { name ... on MarketRegionCountry { code } } } } }
    currencySettings { baseCurrency { currencyCode manualRate } localCurrencies roundingEnabled }
    priceInclusions { inclusiveTaxPricingStrategy }
    catalogs(first: 5) { nodes { id } } webPresences(first: 5) { nodes { id } }";

#[test]
fn the_demo_store_answers_its_markets_without_defaults() {
    let mut shop = demo();
    let answer = run(
        &mut shop,
        &format!(
            "{{ markets(first: 5) {{ nodes {{ {MARKET_FIELDS} }} }}
              webPresences(first: 5) {{ nodes {{ id subfolderSuffix domain {{ host localization {{ defaultLocale alternateLocales country }} }} defaultLocale {{ locale }} alternateLocales {{ locale }} rootUrls {{ locale url }} markets(first: 5) {{ nodes {{ name }} }} }} }}
              shopLocales {{ locale name primary published marketWebPresences {{ id }} }}
              backupRegion {{ id name ... on MarketRegionCountry {{ code }} }}
              marketByGeography(countryCode: FR) {{ name }}
              catalogs(first: 5) {{ nodes {{ id title status publication {{ name autoPublish supportsFuturePublishing }} }} }}
              publications(first: 5) {{ nodes {{ name autoPublish catalog {{ title }} }} }}
              catalogsCount {{ count }} publicationsCount {{ count }}
              marketsResolvedValues(buyerSignal: {{countryCode: FR}}) {{ currencyCode priceInclusivity {{ taxesIncluded dutiesIncluded }} }}
              availableLocales {{ isoCode name }} }}"
        ),
        json!({}),
    );
    assert!(answer.get("errors").is_none(), "{answer}");
    assert!(
        answer.get("extensions").is_none(),
        "defaulted: {}",
        answer["extensions"]
    );
    let data = &answer["data"];
    let names: Vec<&str> = data["markets"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|market| market["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Canada", "France", "United States"], "by name");
    assert_eq!(data["markets"]["nodes"][0]["primary"], json!(true));
    let presence = &data["webPresences"]["nodes"][0];
    assert_eq!(
        presence["rootUrls"],
        json!([
            {"locale": "en", "url": "https://local-supply-co.myshopify.com"},
            {"locale": "fr", "url": "https://local-supply-co.myshopify.com/fr"},
        ])
    );
    assert_eq!(presence["markets"]["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(data["backupRegion"]["code"], json!("CA"));
    assert_eq!(data["marketByGeography"]["name"], json!("France"));
    assert_eq!(data["catalogs"]["nodes"][0]["title"], json!("Online Store"));
    assert_eq!(
        data["publications"]["nodes"][0]["catalog"]["title"],
        json!("Online Store")
    );
    assert_eq!(data["availableLocales"].as_array().unwrap().len(), 137);
    assert_eq!(
        data["availableLocales"][0],
        json!({"isoCode": "af", "name": "Afrikaans"})
    );
}

const MARKET_CREATE: &str = "mutation($input: MarketCreateInput!) { marketCreate(input: $input) {
    market { id name handle type status conditions { conditionTypes regionsCondition { regions(first: 10) { nodes { id name ... on MarketRegionCountry { code } } } } companyLocationsCondition { applicationLevel companyLocations(first: 5) { nodes { id } } } }
      currencySettings { baseCurrency { currencyCode } localCurrencies } catalogs(first: 5) { nodes { id } } webPresences(first: 5) { nodes { id } } }
    userErrors { field message code } } }";

#[test]
fn creates_updates_and_deletes_markets() {
    let mut shop = demo();
    shop.take_events();
    // A new market of a country of no market, with its currency.
    let answer = run(
        &mut shop,
        MARKET_CREATE,
        json!({"input": {"name": "Germany", "handle": "DE", "conditions": {"regionsCondition": {"regions": [{"countryCode": "DE"}]}}}}),
    );
    let market = ok(&answer, "marketCreate")["market"].clone();
    assert_eq!(market["handle"], json!("de"), "written as a handle");
    assert_eq!(market["type"], json!("REGION"));
    assert_eq!(market["status"], json!("ACTIVE"));
    assert_eq!(
        market["conditions"]["regionsCondition"]["regions"]["nodes"][0],
        json!({"id": super::region_gid("DE"), "name": "Germany", "code": "DE"})
    );
    assert_eq!(
        market["currencySettings"],
        json!({"baseCurrency": {"currencyCode": "EUR"}, "localCurrencies": true}),
        "Germany's currency"
    );
    let id = market["id"].as_str().unwrap().to_string();
    assert_eq!(
        id,
        crate::domains::content::market_gid("de"),
        "the id the sync reads"
    );
    assert_eq!(
        events(&mut shop),
        [("markets/create".to_string(), id.clone())]
    );

    // A country of another market: refused, or that market put in draft.
    let input = json!({"input": {"name": "North America", "conditions": {"regionsCondition": {"regions": [{"countryCode": "US"}, {"countryCode": "MX"}]}}}});
    let errors = refused(&mut shop, MARKET_CREATE, input.clone(), "marketCreate");
    assert_eq!(errors[0]["code"], json!("DUPLICATE_UNIQUE_MARKET"));
    let mut drafting = input.clone();
    drafting["input"]["makeDuplicateUniqueMarketsDraft"] = json!(true);
    let answer = run(&mut shop, MARKET_CREATE, drafting);
    let north = ok(&answer, "marketCreate")["market"]["id"].clone();
    let us = market_of(&shop, "US");
    assert_eq!(
        us,
        north.as_str().unwrap(),
        "the US are North America's now"
    );
    assert_eq!(
        topics(&mut shop),
        ["markets/update", "markets/create"],
        "the United States market in draft first"
    );

    // Blank and taken names.
    let errors = refused(
        &mut shop,
        MARKET_CREATE,
        json!({"input": {"name": " "}}),
        "marketCreate",
    );
    assert_eq!(
        errors,
        json!([{"field": ["input", "name"], "message": "Name can't be blank", "code": "BLANK"}])
    );
    let errors = refused(
        &mut shop,
        MARKET_CREATE,
        json!({"input": {"name": "germany"}}),
        "marketCreate",
    );
    assert_eq!(errors[0]["code"], json!("TAKEN"));

    // A B2B market of every company location.
    let answer = run(
        &mut shop,
        MARKET_CREATE,
        json!({"input": {"name": "Company Location Market", "handle": "Company-Location", "conditions": {"companyLocationsCondition": {"applicationLevel": "ALL"}}}}),
    );
    let b2b = ok(&answer, "marketCreate")["market"].clone();
    assert_eq!(b2b["handle"], json!("company-location"));
    assert_eq!(b2b["type"], json!("COMPANY_LOCATION"));
    assert_eq!(
        b2b["conditions"]["companyLocationsCondition"],
        json!({"applicationLevel": "ALL", "companyLocations": {"nodes": []}})
    );
    shop.take_events();

    // Update: a region added, the name, draft.
    let update = "mutation($id: ID!, $input: MarketUpdateInput!) { marketUpdate(id: $id, input: $input) {
        market { name status enabled conditions { regionsCondition { regions(first: 5) { nodes { name } } } } currencySettings { baseCurrency { currencyCode manualRate } localCurrencies } }
        userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        update,
        json!({"id": id, "input": {"name": "DACH", "status": "DRAFT",
            "conditions": {"conditionsToAdd": {"regionsCondition": {"regions": [{"countryCode": "AT"}, {"countryCode": "CH"}]}}},
            "currencySettings": {"baseCurrency": "CHF", "baseCurrencyManualRate": "0.9", "localCurrencies": false}}}),
    );
    let updated = &ok(&answer, "marketUpdate")["market"];
    assert_eq!(updated["name"], json!("DACH"));
    assert_eq!(updated["status"], json!("DRAFT"));
    assert_eq!(updated["enabled"], json!(false));
    assert_eq!(
        updated["conditions"]["regionsCondition"]["regions"]["nodes"],
        json!([{"name": "Germany"}, {"name": "Austria"}, {"name": "Switzerland"}])
    );
    assert_eq!(
        updated["currencySettings"],
        json!({"baseCurrency": {"currencyCode": "CHF", "manualRate": "0.9"}, "localCurrencies": false})
    );
    assert_eq!(topics(&mut shop), ["markets/update"]);
    let errors = refused(
        &mut shop,
        update,
        json!({"id": id, "input": {"status": "ACTIVE", "enabled": false}}),
        "marketUpdate",
    );
    assert_eq!(
        errors[0]["code"],
        json!("INVALID_STATUS_AND_ENABLED_COMBINATION")
    );
    let canada = market_of(&shop, "CA");
    let errors = refused(
        &mut shop,
        update,
        json!({"id": canada, "input": {"status": "DRAFT"}}),
        "marketUpdate",
    );
    assert_eq!(errors[0]["code"], json!("CANNOT_DISABLE_PRIMARY_MARKET"));
    let errors = refused(
        &mut shop,
        update,
        json!({"id": "gid://shopify/Market/1", "input": {"name": "X"}}),
        "marketUpdate",
    );
    assert_eq!(errors[0]["code"], json!("MARKET_NOT_FOUND"));

    // Delete.
    let delete = "mutation($id: ID!) { marketDelete(id: $id) { deletedId userErrors { field message code } } }";
    let errors = refused(&mut shop, delete, json!({"id": canada}), "marketDelete");
    assert_eq!(errors[0]["code"], json!("CANNOT_DELETE_PRIMARY_MARKET"));
    let answer = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(ok(&answer, "marketDelete")["deletedId"], json!(id));
    assert!(!shop.contains(&id));
    let deleted = shop.take_events();
    assert_eq!(deleted[0].topic, "markets/delete");
    assert!(deleted[0].snapshot.is_some(), "the payload's record");
}

#[test]
fn the_deprecated_region_and_currency_mutations_change_a_market() {
    let mut shop = demo();
    let france = market_of(&shop, "FR");
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { marketRegionsCreate(marketId: $id, regions: [{countryCode: BE}, {countryCode: LU}]) { market { regions(first: 5) { nodes { id name } } } userErrors { field message code } } }",
        json!({"id": france}),
    );
    let regions = ok(&answer, "marketRegionsCreate")["market"]["regions"]["nodes"].clone();
    assert_eq!(regions.as_array().unwrap().len(), 3);
    assert_eq!(topics(&mut shop), ["markets/update"]);
    // A country another market has.
    let errors = refused(
        &mut shop,
        "mutation($id: ID!) { marketRegionsCreate(marketId: $id, regions: [{countryCode: US}]) { market { id } userErrors { field message code } } }",
        json!({"id": france}),
        "marketRegionsCreate",
    );
    assert_eq!(errors[0]["code"], json!("DUPLICATE_UNIQUE_MARKET"));
    // Deleting one region, then two of which one is a market's only one.
    let belgium = regions[1]["id"].clone();
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { marketRegionDelete(id: $id) { deletedId market { regions(first: 5) { nodes { name } } } userErrors { field message code } } }",
        json!({"id": belgium}),
    );
    assert_eq!(ok(&answer, "marketRegionDelete")["deletedId"], belgium);
    let us_region = super::region_gid("US");
    let errors = refused(
        &mut shop,
        "mutation($ids: [ID!]!) { marketRegionsDelete(ids: $ids) { deletedIds userErrors { field message code } } }",
        json!({"ids": [regions[2]["id"], us_region]}),
        "marketRegionsDelete",
    );
    assert_eq!(
        errors,
        json!([{"field": ["ids", "1"], "message": "Can't delete the only region in a market.", "code": "CANNOT_DELETE_ONLY_REGION"}])
    );
    let answer = run(
        &mut shop,
        "mutation($ids: [ID!]!) { marketRegionsDelete(ids: $ids) { deletedIds userErrors { field message code } } }",
        json!({"ids": [regions[2]["id"]]}),
    );
    assert_eq!(
        ok(&answer, "marketRegionsDelete")["deletedIds"],
        json!([regions[2]["id"]])
    );
    shop.take_events();
    // Currency settings.
    let currency = "mutation($id: ID!, $input: MarketCurrencySettingsUpdateInput!) { marketCurrencySettingsUpdate(marketId: $id, input: $input) { market { currencySettings { baseCurrency { currencyCode } localCurrencies } } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        currency,
        json!({"id": france, "input": {"baseCurrency": "EUR"}}),
    );
    assert_eq!(
        ok(&answer, "marketCurrencySettingsUpdate")["market"]["currencySettings"]["baseCurrency"],
        json!({"currencyCode": "EUR"})
    );
    assert_eq!(topics(&mut shop), ["markets/update"]);
    let errors = refused(
        &mut shop,
        currency,
        json!({"id": france, "input": {"localCurrencies": true}}),
        "marketCurrencySettingsUpdate",
    );
    assert_eq!(
        errors[0]["code"],
        json!("NO_LOCAL_CURRENCIES_ON_SINGLE_COUNTRY_MARKET")
    );
}

#[test]
fn updates_the_backup_region() {
    let mut shop = demo();
    shop.take_events();
    let update = "mutation($region: BackupRegionUpdateInput) { backupRegionUpdate(region: $region) { backupRegion { id name ... on MarketRegionCountry { code } } userErrors { field message code } } }";
    let answer = run(&mut shop, update, json!({"region": {"countryCode": "FR"}}));
    assert_eq!(
        ok(&answer, "backupRegionUpdate")["backupRegion"],
        json!({"id": super::region_gid("FR"), "name": "France", "code": "FR"})
    );
    let emitted = shop.take_events();
    assert_eq!(emitted[0].topic, "markets_backup_region/update");
    let answer = run(&mut shop, "{ backupRegion { name } }", json!({}));
    assert_eq!(answer["data"]["backupRegion"]["name"], json!("France"));
    let errors = refused(
        &mut shop,
        update,
        json!({"region": {"countryCode": "JP"}}),
        "backupRegionUpdate",
    );
    assert_eq!(errors[0]["field"], json!(["region", "countryCode"]));
}

const WEB_PRESENCE: &str = "webPresence { id subfolderSuffix domain { id } defaultLocale { locale } alternateLocales { locale } rootUrls { locale url } }";

#[test]
fn creates_updates_and_deletes_web_presences() {
    let mut shop = demo();
    shop.take_events();
    let create = format!(
        "mutation($input: WebPresenceCreateInput!) {{ webPresenceCreate(input: $input) {{ {WEB_PRESENCE} userErrors {{ field message code }} }} }}"
    );
    let answer = run(
        &mut shop,
        &create,
        json!({"input": {"defaultLocale": "fr", "alternateLocales": ["en"], "subfolderSuffix": "CA"}}),
    );
    let presence = ok(&answer, "webPresenceCreate")["webPresence"].clone();
    assert_eq!(presence["subfolderSuffix"], json!("ca"));
    assert_eq!(
        presence["rootUrls"],
        json!([
            {"locale": "fr", "url": "https://local-supply-co.myshopify.com/fr-ca"},
            {"locale": "en", "url": "https://local-supply-co.myshopify.com/en-ca"},
        ])
    );
    let id = presence["id"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["market_web_presences/create"]);
    // Errors of the input.
    for (input, code) in [
        (
            json!({"defaultLocale": "fr"}),
            "REQUIRES_DOMAIN_OR_SUBFOLDER",
        ),
        (
            json!({"defaultLocale": "fr", "subfolderSuffix": "c"}),
            "SUBFOLDER_SUFFIX_MUST_BE_AT_LEAST_2_LETTERS",
        ),
        (
            json!({"defaultLocale": "fr", "subfolderSuffix": "Latn"}),
            "SUBFOLDER_SUFFIX_CANNOT_BE_SCRIPT_CODE",
        ),
        (
            json!({"defaultLocale": "fr", "subfolderSuffix": "ca"}),
            "TAKEN",
        ),
        (
            json!({"defaultLocale": "de", "subfolderSuffix": "de"}),
            "DISABLED_LANGUAGE",
        ),
        (
            json!({"defaultLocale": "fr", "subfolderSuffix": "be", "alternateLocales": ["fr"]}),
            "DUPLICATE_LANGUAGES",
        ),
        (
            json!({"defaultLocale": "fr", "domainId": "gid://shopify/Domain/1"}),
            "DOMAIN_NOT_FOUND",
        ),
    ] {
        let errors = refused(
            &mut shop,
            &create,
            json!({"input": input}),
            "webPresenceCreate",
        );
        assert_eq!(errors[0]["code"], json!(code), "{errors}");
    }
    // Linked to a market, the market lists it.
    let france = market_of(&shop, "FR");
    let answer = run(
        &mut shop,
        "mutation($id: ID!, $presence: ID!) { marketUpdate(id: $id, input: {webPresencesToAdd: [$presence]}) { market { webPresences(first: 5) { nodes { id } } } userErrors { message } } }",
        json!({"id": france, "presence": id}),
    );
    assert_eq!(
        ok(&answer, "marketUpdate")["market"]["webPresences"]["nodes"][1]["id"],
        json!(id)
    );
    shop.take_events();
    let update = format!(
        "mutation($id: ID!, $input: WebPresenceUpdateInput!) {{ webPresenceUpdate(id: $id, input: $input) {{ {WEB_PRESENCE} userErrors {{ field message code }} }} }}"
    );
    let answer = run(
        &mut shop,
        &update,
        json!({"id": id, "input": {"alternateLocales": []}}),
    );
    assert_eq!(
        ok(&answer, "webPresenceUpdate")["webPresence"]["rootUrls"],
        json!([{"locale": "fr", "url": "https://local-supply-co.myshopify.com/fr-ca"}])
    );
    assert_eq!(topics(&mut shop), ["market_web_presences/update"]);
    refused(
        &mut shop,
        &update,
        json!({"id": id, "input": {"defaultLocale": "ja"}}),
        "webPresenceUpdate",
    );
    // The primary domain's presence: its locales change its domain's localization.
    let primary = shop.keys("MarketWebPresence").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        &update,
        json!({"id": primary, "input": {"alternateLocales": []}}),
    );
    ok(&answer, "webPresenceUpdate");
    let emitted = events(&mut shop);
    assert!(
        emitted.iter().any(|(topic, _)| topic == "domains/update"),
        "{emitted:?}"
    );
    let delete = "mutation($id: ID!) { webPresenceDelete(id: $id) { deletedId userErrors { field message code } } }";
    let errors = refused(
        &mut shop,
        delete,
        json!({"id": primary}),
        "webPresenceDelete",
    );
    assert_eq!(
        errors[0]["code"],
        json!("SHOP_MUST_HAVE_PRIMARY_DOMAIN_WEB_PRESENCE")
    );
    let answer = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(ok(&answer, "webPresenceDelete")["deletedId"], json!(id));
    let answer = run(
        &mut shop,
        "query($id: ID!) { market(id: $id) { webPresences(first: 5) { nodes { id } } } }",
        json!({"id": france}),
    );
    assert_eq!(
        answer["data"]["market"]["webPresences"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn the_deprecated_market_web_presence_mutations() {
    let mut shop = demo();
    let france = market_of(&shop, "FR");
    let create = "mutation($id: ID!, $presence: MarketWebPresenceCreateInput!) { marketWebPresenceCreate(marketId: $id, webPresence: $presence) { market { id webPresences(first: 5) { nodes { id subfolderSuffix } } } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        create,
        json!({"id": france, "presence": {"defaultLocale": "fr", "subfolderSuffix": "fr"}}),
    );
    let presences =
        ok(&answer, "marketWebPresenceCreate")["market"]["webPresences"]["nodes"].clone();
    let id = presences[1]["id"].clone();
    assert_eq!(presences[1]["subfolderSuffix"], json!("fr"));
    refused(
        &mut shop,
        create,
        json!({"id": "gid://shopify/Market/1", "presence": {"defaultLocale": "fr", "subfolderSuffix": "be"}}),
        "marketWebPresenceCreate",
    );
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { marketWebPresenceUpdate(webPresenceId: $id, webPresence: {subfolderSuffix: \"eu\"}) { market { id } userErrors { field message code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        ok(&answer, "marketWebPresenceUpdate")["market"]["id"],
        json!(france)
    );
    refused(
        &mut shop,
        "mutation($id: ID!) { marketWebPresenceUpdate(webPresenceId: $id, webPresence: {subfolderSuffix: \"x\"}) { market { id } userErrors { field message code } } }",
        json!({"id": id}),
        "marketWebPresenceUpdate",
    );
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { marketWebPresenceDelete(webPresenceId: $id) { deletedId market { id } userErrors { field message code } } }",
        json!({"id": id}),
    );
    assert_eq!(ok(&answer, "marketWebPresenceDelete")["deletedId"], id);
    refused(
        &mut shop,
        "mutation($id: ID!) { marketWebPresenceDelete(webPresenceId: $id) { deletedId userErrors { field message code } } }",
        json!({"id": id}),
        "marketWebPresenceDelete",
    );
}

#[test]
fn enables_updates_and_disables_locales() {
    let mut shop = demo();
    shop.take_events();
    let enable = "mutation($locale: String!) { shopLocaleEnable(locale: $locale) { shopLocale { locale name primary published } userErrors { message field } } }";
    let answer = run(&mut shop, enable, json!({"locale": "ko"}));
    assert_eq!(
        ok(&answer, "shopLocaleEnable")["shopLocale"],
        json!({"locale": "ko", "name": "Korean", "primary": false, "published": false}),
        "the reference's example"
    );
    assert_eq!(
        events(&mut shop),
        [("locales/create".to_string(), "ko".to_string())]
    );
    refused(
        &mut shop,
        enable,
        json!({"locale": "ko"}),
        "shopLocaleEnable",
    );
    refused(
        &mut shop,
        enable,
        json!({"locale": "xx"}),
        "shopLocaleEnable",
    );
    let update = "mutation($locale: String!, $input: ShopLocaleInput!) { shopLocaleUpdate(locale: $locale, shopLocale: $input) { shopLocale { locale published marketWebPresences { id } } userErrors { message field } } }";
    let presence = shop.keys("MarketWebPresence").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        update,
        json!({"locale": "ko", "input": {"published": true, "marketWebPresenceIds": [presence]}}),
    );
    assert_eq!(
        ok(&answer, "shopLocaleUpdate")["shopLocale"],
        json!({"locale": "ko", "published": true, "marketWebPresences": [{"id": presence}]})
    );
    let emitted = topics(&mut shop);
    assert!(
        emitted.contains(&"locales/update".to_string()),
        "{emitted:?}"
    );
    let answer = run(
        &mut shop,
        "{ shopLocales(published: true) { locale } }",
        json!({}),
    );
    assert_eq!(
        answer["data"]["shopLocales"],
        json!([{"locale": "en"}, {"locale": "fr"}, {"locale": "ko"}])
    );
    let errors = refused(
        &mut shop,
        update,
        json!({"locale": "en", "input": {"published": false}}),
        "shopLocaleUpdate",
    );
    assert_eq!(errors[0]["field"], json!(["shopLocale", "published"]));
    refused(
        &mut shop,
        update,
        json!({"locale": "ja", "input": {"published": true}}),
        "shopLocaleUpdate",
    );
    // A translation in the locale, which disabling it deletes.
    let product = shop.keys("Product").next().unwrap().to_string();
    let title = shop.get(&product).unwrap()["title"]
        .as_str()
        .unwrap()
        .to_string();
    let digest: String = sha2::Digest::finalize(sha2::Digest::chain_update(
        <sha2::Sha256 as sha2::Digest>::new(),
        title.as_bytes(),
    ))
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect();
    let answer = run(
        &mut shop,
        "mutation($id: ID!, $digest: String!) { translationsRegister(resourceId: $id, translations: [{locale: \"ko\", key: \"title\", value: \"티셔츠\", translatableContentDigest: $digest}]) { userErrors { message } } }",
        json!({"id": product, "digest": digest}),
    );
    ok(&answer, "translationsRegister");
    shop.take_events();
    let disable = "mutation($locale: String!) { shopLocaleDisable(locale: $locale) { locale userErrors { message field } } }";
    refused(
        &mut shop,
        disable,
        json!({"locale": "en"}),
        "shopLocaleDisable",
    );
    let answer = run(&mut shop, disable, json!({"locale": "ko"}));
    assert_eq!(ok(&answer, "shopLocaleDisable")["locale"], json!("ko"));
    assert!(!shop.contains("ko"));
    assert!(
        !shop.contains(&format!("translations/{product}")),
        "its translations go"
    );
    let emitted = shop.take_events();
    let destroyed = emitted
        .iter()
        .find(|event| event.topic == "locales/destroy")
        .unwrap();
    assert!(destroyed.snapshot.is_some());
    let answer = run(
        &mut shop,
        "query($id: ID!) { webPresence: node(id: $id) { ... on MarketWebPresence { alternateLocales { locale } } } }",
        json!({"id": presence}),
    );
    assert_eq!(
        answer["data"]["webPresence"]["alternateLocales"],
        json!([{"locale": "fr"}])
    );
    refused(
        &mut shop,
        disable,
        json!({"locale": "ko"}),
        "shopLocaleDisable",
    );
}

#[test]
fn registers_and_removes_market_localizations() {
    let mut shop = rich();
    let market = shop.keys("Market").next().unwrap().to_string();
    let metafield = shop
        .keys("Metafield")
        .find(|key| shop.get(key).unwrap()["value"].is_string())
        .unwrap()
        .to_string();
    let value = shop.get(&metafield).unwrap()["value"]
        .as_str()
        .unwrap()
        .to_string();
    let answer = run(
        &mut shop,
        "query($id: ID!) { marketLocalizableResource(resourceId: $id) { resourceId marketLocalizableContent { key value digest } } }",
        json!({"id": metafield}),
    );
    let content = &answer["data"]["marketLocalizableResource"]["marketLocalizableContent"][0];
    assert_eq!(content["key"], json!("value"));
    assert_eq!(content["value"], json!(value));
    let digest = content["digest"].as_str().unwrap().to_string();
    shop.take_events();
    let register = "mutation($id: ID!, $localizations: [MarketLocalizationRegisterInput!]!) { marketLocalizationsRegister(resourceId: $id, marketLocalizations: $localizations) { marketLocalizations { key value outdated market { id } } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        register,
        json!({"id": metafield, "localizations": [{"key": "value", "value": "local", "marketId": market, "marketLocalizableContentDigest": digest}]}),
    );
    assert_eq!(
        ok(&answer, "marketLocalizationsRegister")["marketLocalizations"],
        json!([{"key": "value", "value": "local", "outdated": false, "market": {"id": market}}])
    );
    assert_eq!(topics(&mut shop), ["market_localizations/update"]);
    let errors = refused(
        &mut shop,
        register,
        json!({"id": metafield, "localizations": [
            {"key": "title", "value": "x", "marketId": market, "marketLocalizableContentDigest": digest},
            {"key": "value", "value": "x", "marketId": "gid://shopify/Market/1", "marketLocalizableContentDigest": "nope"},
        ]}),
        "marketLocalizationsRegister",
    );
    let codes: Vec<&str> = errors
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        [
            "INVALID_KEY_FOR_MODEL",
            "MARKET_DOES_NOT_EXIST",
            "INVALID_MARKET_LOCALIZABLE_CONTENT"
        ]
    );
    let answer = run(
        &mut shop,
        "query($id: ID!, $market: ID!) { marketLocalizableResource(resourceId: $id) { marketLocalizations(marketId: $market) { key value } } }",
        json!({"id": metafield, "market": market}),
    );
    assert_eq!(
        answer["data"]["marketLocalizableResource"]["marketLocalizations"],
        json!([{"key": "value", "value": "local"}])
    );
    let remove = "mutation($id: ID!, $markets: [ID!]!) { marketLocalizationsRemove(resourceId: $id, marketIds: $markets, marketLocalizationKeys: [\"value\"]) { marketLocalizations { key value } userErrors { field message code } } }";
    refused(
        &mut shop,
        remove,
        json!({"id": metafield, "markets": ["gid://shopify/Market/1"]}),
        "marketLocalizationsRemove",
    );
    let answer = run(
        &mut shop,
        remove,
        json!({"id": metafield, "markets": [market]}),
    );
    assert_eq!(
        ok(&answer, "marketLocalizationsRemove")["marketLocalizations"],
        json!([{"key": "value", "value": "local"}])
    );
    assert!(!shop.contains(&format!("market_localizations/{metafield}")));
    let answer = run(
        &mut shop,
        "{ marketLocalizableResources(resourceType: METAFIELD, first: 1) { nodes { resourceId } } }",
        json!({}),
    );
    assert!(answer["data"]["marketLocalizableResources"]["nodes"][0]["resourceId"].is_string());
}

const CATALOG_CREATE: &str = "mutation($input: CatalogCreateInput!) { catalogCreate(input: $input) { catalog { id title status __typename priceList { id currency } publication { id autoPublish } ... on MarketCatalog { markets(first: 5) { nodes { id } } } ... on CompanyLocationCatalog { companyLocations(first: 5) { nodes { id } } } } userErrors { field message code } } }";

#[test]
fn creates_updates_and_deletes_catalogs() {
    let mut shop = rich();
    shop.take_events();
    let europe = shop.keys("Market").next().unwrap().to_string();
    let location = shop.keys("CompanyLocation").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        CATALOG_CREATE,
        json!({"input": {"title": "Europe catalog", "status": "ACTIVE", "context": {"marketIds": [europe]}}}),
    );
    let catalog = ok(&answer, "catalogCreate")["catalog"].clone();
    assert_eq!(catalog["__typename"], json!("MarketCatalog"));
    assert_eq!(catalog["markets"]["nodes"], json!([{"id": europe}]));
    let id = catalog["id"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["catalogs/create"]);
    // `Catalog/<n>` names it, and the market lists it.
    let number = crate::state::parse_gid(&id).unwrap().1;
    let answer = run(
        &mut shop,
        "query($id: ID!, $market: ID!) { catalog(id: $id) { id } market(id: $market) { catalogs(first: 5) { nodes { id } } catalogsCount { count } } catalogs(first: 5, type: MARKET) { nodes { id } } catalogsCount(type: APP) { count } }",
        json!({"id": format!("gid://shopify/Catalog/{number}"), "market": europe}),
    );
    assert_eq!(answer["data"]["catalog"]["id"], json!(id));
    assert_eq!(
        answer["data"]["market"]["catalogs"]["nodes"],
        json!([{"id": id}])
    );
    assert_eq!(answer["data"]["catalogs"]["nodes"], json!([{"id": id}]));
    assert_eq!(
        answer["data"]["catalogsCount"]["count"],
        json!(1),
        "the Online Store's"
    );
    for (input, code) in [
        (
            json!({"title": "", "status": "ACTIVE", "context": {"marketIds": [europe]}}),
            "BLANK",
        ),
        (
            json!({"title": "Europe catalog", "status": "ACTIVE", "context": {"marketIds": [europe]}}),
            "TAKEN",
        ),
        (
            json!({"title": "Both", "status": "ACTIVE", "context": {"marketIds": [europe], "companyLocationIds": [location]}}),
            "MUST_PROVIDE_EXACTLY_ONE_CONTEXT_TYPE",
        ),
        (
            json!({"title": "Nowhere", "status": "ACTIVE", "context": {"marketIds": ["gid://shopify/Market/1"]}}),
            "MARKET_NOT_FOUND",
        ),
        (
            json!({"title": "No list", "status": "ACTIVE", "context": {"marketIds": [europe]}, "priceListId": "gid://shopify/PriceList/1"}),
            "PRICE_LIST_NOT_FOUND",
        ),
    ] {
        let errors = refused(
            &mut shop,
            CATALOG_CREATE,
            json!({"input": input}),
            "catalogCreate",
        );
        assert_eq!(errors[0]["code"], json!(code), "{errors}");
    }
    // A B2B catalog, its context changed.
    let answer = run(
        &mut shop,
        CATALOG_CREATE,
        json!({"input": {"title": "B2B", "status": "DRAFT", "context": {"companyLocationIds": [location]}}}),
    );
    let b2b = ok(&answer, "catalogCreate")["catalog"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let answer = run(
        &mut shop,
        "query($id: ID!, $catalog: ID!) { companyLocation(id: $id) { catalogs(first: 5) { nodes { id } } catalogsCount { count } inCatalog(catalogId: $catalog) market { id } } }",
        json!({"id": location, "catalog": b2b}),
    );
    assert_eq!(answer["data"]["companyLocation"]["inCatalog"], json!(true));
    assert_eq!(
        answer["data"]["companyLocation"]["catalogsCount"]["count"],
        json!(1)
    );
    shop.take_events();
    let context = "mutation($id: ID!, $add: CatalogContextInput, $remove: CatalogContextInput) { catalogContextUpdate(catalogId: $id, contextsToAdd: $add, contextsToRemove: $remove) { catalog { ... on CompanyLocationCatalog { companyLocations(first: 5) { nodes { id } } } } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        context,
        json!({"id": b2b, "remove": {"companyLocationIds": [location]}}),
    );
    assert_eq!(
        ok(&answer, "catalogContextUpdate")["catalog"]["companyLocations"]["nodes"],
        json!([])
    );
    assert_eq!(topics(&mut shop), ["catalogs/update"]);
    let errors = refused(
        &mut shop,
        context,
        json!({"id": b2b}),
        "catalogContextUpdate",
    );
    assert_eq!(
        errors[0]["code"],
        json!("REQUIRES_CONTEXTS_TO_ADD_OR_REMOVE")
    );
    let errors = refused(
        &mut shop,
        context,
        json!({"id": b2b, "add": {"marketIds": [europe]}}),
        "catalogContextUpdate",
    );
    assert_eq!(errors[0]["code"], json!("INVALID_CATALOG_CONTEXT_TYPE"));
    // Update, and the app catalog that cannot be.
    let update = "mutation($id: ID!, $input: CatalogUpdateInput!) { catalogUpdate(id: $id, input: $input) { catalog { title status } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        update,
        json!({"id": b2b, "input": {"title": "Wholesale", "status": "ACTIVE"}}),
    );
    assert_eq!(
        ok(&answer, "catalogUpdate")["catalog"],
        json!({"title": "Wholesale", "status": "ACTIVE"})
    );
    let errors = refused(
        &mut shop,
        update,
        json!({"id": super::online_store_catalog_gid(), "input": {"title": "X"}}),
        "catalogUpdate",
    );
    assert_eq!(errors[0]["code"], json!("CANNOT_MODIFY_APP_CATALOG"));
    // Delete, with what depends on it.
    let answer = run(
        &mut shop,
        "mutation { priceListCreate(input: {name: \"Wholesale\", currency: EUR, parent: {adjustment: {type: PERCENTAGE_DECREASE, value: 10}}}) { priceList { id } userErrors { message } } }",
        json!({}),
    );
    let list = ok(&answer, "priceListCreate")["priceList"]["id"].clone();
    let answer = run(
        &mut shop,
        update,
        json!({"id": b2b, "input": {"priceListId": list}}),
    );
    ok(&answer, "catalogUpdate");
    let delete = "mutation($id: ID!, $all: Boolean) { catalogDelete(id: $id, deleteDependentResources: $all) { deletedId userErrors { field message code } } }";
    let errors = refused(
        &mut shop,
        delete,
        json!({"id": super::online_store_catalog_gid()}),
        "catalogDelete",
    );
    assert_eq!(errors[0]["code"], json!("CANNOT_DELETE_APP_CATALOG"));
    let answer = run(&mut shop, delete, json!({"id": b2b, "all": true}));
    assert_eq!(ok(&answer, "catalogDelete")["deletedId"], json!(b2b));
    assert!(
        !shop.contains(list.as_str().unwrap()),
        "its price list went with it"
    );
    let answer = run(&mut shop, delete, json!({"id": id}));
    ok(&answer, "catalogDelete");
}

const PRICE_LIST_FIELDS: &str = "id name currency fixedPricesCount parent { adjustment { type value } settings { compareAtMode } } catalog { id }";

#[test]
fn creates_updates_and_deletes_price_lists_and_their_fixed_prices() {
    let mut shop = demo();
    shop.take_events();
    let (product, variant) = first_variant(&shop);
    let base = crate::domains::catalog::cents_of(shop.get(&variant).unwrap().get("price")).unwrap();
    let create = format!(
        "mutation($input: PriceListCreateInput!) {{ priceListCreate(input: $input) {{ priceList {{ {PRICE_LIST_FIELDS} }} userErrors {{ field message code }} }} }}"
    );
    let answer = run(
        &mut shop,
        &create,
        json!({"input": {"name": "Price List", "currency": "USD", "parent": {"adjustment": {"type": "PERCENTAGE_INCREASE", "value": 10}}}}),
    );
    let list = ok(&answer, "priceListCreate")["priceList"].clone();
    assert_eq!(
        list["parent"]["adjustment"],
        json!({"type": "PERCENTAGE_INCREASE", "value": 10.0})
    );
    assert_eq!(
        list["parent"]["settings"]["compareAtMode"],
        json!("ADJUSTED")
    );
    let id = list["id"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["price_lists/create"]);
    for (input, code) in [
        (
            json!({"name": "Price List", "currency": "USD", "parent": {"adjustment": {"type": "PERCENTAGE_INCREASE", "value": 10}}}),
            "TAKEN",
        ),
        (
            json!({"name": "", "currency": "USD", "parent": {"adjustment": {"type": "PERCENTAGE_INCREASE", "value": 10}}}),
            "BLANK",
        ),
        (
            json!({"name": "Too much", "currency": "USD", "parent": {"adjustment": {"type": "PERCENTAGE_DECREASE", "value": 120}}}),
            "INVALID_ADJUSTMENT_MAX_VALUE",
        ),
        (
            json!({"name": "Negative", "currency": "USD", "parent": {"adjustment": {"type": "PERCENTAGE_DECREASE", "value": -1}}}),
            "INVALID_ADJUSTMENT_MIN_VALUE",
        ),
        (
            json!({"name": "Nowhere", "currency": "USD", "catalogId": "gid://shopify/MarketCatalog/1", "parent": {"adjustment": {"type": "PERCENTAGE_DECREASE", "value": 1}}}),
            "CATALOG_DOES_NOT_EXIST",
        ),
    ] {
        let errors = refused(
            &mut shop,
            &create,
            json!({"input": input}),
            "priceListCreate",
        );
        assert_eq!(errors[0]["code"], json!(code), "{errors}");
    }
    // The relative price: 10% up.
    let prices = "query($id: ID!, $variant: String!) { priceList(id: $id) { fixedPricesCount prices(first: 1, query: $variant) { nodes { originType price { amount currencyCode } variant { id } } } } }";
    let number = crate::state::parse_gid(&variant).unwrap().1;
    let answer = run(
        &mut shop,
        prices,
        json!({"id": id, "variant": format!("variant_id:{number}")}),
    );
    let price = &answer["data"]["priceList"]["prices"]["nodes"][0];
    assert_eq!(price["originType"], json!("RELATIVE"));
    assert_eq!(
        price["price"]["amount"],
        json!(crate::state::value::decimal((base * 11 + 5) / 10))
    );
    // Fixed prices: Shopify's example error, then a fixed price.
    let add = "mutation($id: ID!, $prices: [PriceListPriceInput!]!) { priceListFixedPricesAdd(priceListId: $id, prices: $prices) { prices { compareAtPrice { amount currencyCode } price { amount currencyCode } originType } userErrors { field code message } } }";
    let errors = refused(
        &mut shop,
        add,
        json!({"id": id, "prices": [{"price": {"amount": "100", "currencyCode": "EUR"}, "compareAtPrice": {"amount": "125.50", "currencyCode": "EUR"}, "variantId": variant}]}),
        "priceListFixedPricesAdd",
    );
    assert_eq!(
        errors,
        json!([
            {"field": ["prices", "0", "price", "currencyCode"], "code": "PRICE_LIST_CURRENCY_MISMATCH", "message": "The specified currency does not match the price list's currency."},
            {"field": ["prices", "0", "compareAtPrice", "currencyCode"], "code": "PRICE_LIST_CURRENCY_MISMATCH", "message": "The specified currency does not match the price list's currency."},
        ])
    );
    let answer = run(
        &mut shop,
        add,
        json!({"id": id, "prices": [{"price": {"amount": "100", "currencyCode": "USD"}, "compareAtPrice": {"amount": "125.50", "currencyCode": "USD"}, "variantId": variant}]}),
    );
    assert_eq!(
        ok(&answer, "priceListFixedPricesAdd")["prices"],
        json!([{"compareAtPrice": {"amount": "125.5", "currencyCode": "USD"}, "price": {"amount": "100.0", "currencyCode": "USD"}, "originType": "FIXED"}]),
        "the amounts as the example writes them"
    );
    assert_eq!(topics(&mut shop), ["price_lists/update"]);
    let answer = run(
        &mut shop,
        prices,
        json!({"id": id, "variant": format!("variant_id:{number}")}),
    );
    assert_eq!(answer["data"]["priceList"]["fixedPricesCount"], json!(1));
    assert_eq!(
        answer["data"]["priceList"]["prices"]["nodes"][0]["originType"],
        json!("FIXED")
    );
    // Update: add and delete in one.
    let other = crate::domains::catalog::gids(shop.get(&product).unwrap().get("variants"))
        .get(1)
        .cloned()
        .unwrap_or_else(|| variant.clone());
    let update = "mutation($id: ID!, $add: [PriceListPriceInput!]!, $delete: [ID!]!) { priceListFixedPricesUpdate(priceListId: $id, pricesToAdd: $add, variantIdsToDelete: $delete) { deletedFixedPriceVariantIds pricesAdded { price { amount } } priceList { fixedPricesCount } userErrors { field code message } } }";
    let errors = refused(
        &mut shop,
        update,
        json!({"id": id, "add": [], "delete": ["gid://shopify/ProductVariant/1"]}),
        "priceListFixedPricesUpdate",
    );
    assert_eq!(errors[0]["code"], json!("VARIANT_NOT_FOUND"));
    let answer = run(
        &mut shop,
        update,
        json!({"id": id, "add": [{"price": {"amount": "5", "currencyCode": "USD"}, "variantId": other}], "delete": [variant]}),
    );
    let payload = ok(&answer, "priceListFixedPricesUpdate");
    assert_eq!(payload["deletedFixedPriceVariantIds"], json!([variant]));
    assert_eq!(
        payload["pricesAdded"],
        json!([{"price": {"amount": "5.0"}}])
    );
    // By product: every variant of it.
    let by_product = "mutation($id: ID!, $add: [PriceListProductPriceInput!], $delete: [ID!]) { priceListFixedPricesByProductUpdate(priceListId: $id, pricesToAdd: $add, pricesToDeleteByProductIds: $delete) { pricesToAddProducts { id } pricesToDeleteProducts { id } priceList { fixedPricesCount } userErrors { field code message } } }";
    let errors = refused(
        &mut shop,
        by_product,
        json!({"id": id}),
        "priceListFixedPricesByProductUpdate",
    );
    assert_eq!(errors[0]["code"], json!("NO_UPDATE_OPERATIONS_SPECIFIED"));
    let errors = refused(
        &mut shop,
        by_product,
        json!({"id": id, "add": [{"productId": product, "price": {"amount": "7", "currencyCode": "USD"}}], "delete": [product]}),
        "priceListFixedPricesByProductUpdate",
    );
    assert_eq!(errors[0]["code"], json!("ID_MUST_BE_MUTUALLY_EXCLUSIVE"));
    let answer = run(
        &mut shop,
        by_product,
        json!({"id": id, "add": [{"productId": product, "price": {"amount": "7", "currencyCode": "USD"}}]}),
    );
    let payload = ok(&answer, "priceListFixedPricesByProductUpdate");
    assert_eq!(payload["pricesToAddProducts"], json!([{"id": product}]));
    let count = crate::domains::catalog::gids(shop.get(&product).unwrap().get("variants")).len();
    assert_eq!(payload["priceList"]["fixedPricesCount"], json!(count));
    let answer = run(
        &mut shop,
        by_product,
        json!({"id": id, "delete": [product]}),
    );
    assert_eq!(
        ok(&answer, "priceListFixedPricesByProductUpdate")["priceList"]["fixedPricesCount"],
        json!(0)
    );
    // Delete fixed prices.
    let answer = run(
        &mut shop,
        add,
        json!({"id": id, "prices": [{"price": {"amount": "9", "currencyCode": "USD"}, "variantId": variant}]}),
    );
    ok(&answer, "priceListFixedPricesAdd");
    let delete_prices = "mutation($id: ID!, $variants: [ID!]!) { priceListFixedPricesDelete(priceListId: $id, variantIds: $variants) { deletedFixedPriceVariantIds userErrors { field code message } } }";
    let errors = refused(
        &mut shop,
        delete_prices,
        json!({"id": id, "variants": [other]}),
        "priceListFixedPricesDelete",
    );
    assert_eq!(errors[0]["code"], json!("PRICE_NOT_FIXED"));
    let answer = run(
        &mut shop,
        delete_prices,
        json!({"id": id, "variants": [variant]}),
    );
    assert_eq!(
        ok(&answer, "priceListFixedPricesDelete")["deletedFixedPriceVariantIds"],
        json!([variant])
    );
    // Update the list: a new currency drops the fixed prices.
    let answer = run(
        &mut shop,
        add,
        json!({"id": id, "prices": [{"price": {"amount": "9", "currencyCode": "USD"}, "variantId": variant}]}),
    );
    ok(&answer, "priceListFixedPricesAdd");
    let update_list = format!(
        "mutation($id: ID!, $input: PriceListUpdateInput!) {{ priceListUpdate(id: $id, input: $input) {{ priceList {{ {PRICE_LIST_FIELDS} }} userErrors {{ field message code }} }} }}"
    );
    let answer = run(
        &mut shop,
        &update_list,
        json!({"id": id, "input": {"currency": "CAD", "parent": {"adjustment": {"type": "PERCENTAGE_DECREASE", "value": 5}, "settings": {"compareAtMode": "NULLIFY"}}}}),
    );
    let updated = &ok(&answer, "priceListUpdate")["priceList"];
    assert_eq!(updated["currency"], json!("CAD"));
    assert_eq!(updated["fixedPricesCount"], json!(0));
    assert_eq!(
        updated["parent"]["settings"]["compareAtMode"],
        json!("NULLIFY")
    );
    refused(
        &mut shop,
        &update_list,
        json!({"id": id, "input": {"name": ""}}),
        "priceListUpdate",
    );
    // Delete the list.
    let delete = "mutation($id: ID!) { priceListDelete(id: $id) { deletedId userErrors { field code message } } }";
    let answer = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(ok(&answer, "priceListDelete")["deletedId"], json!(id));
    let errors = refused(&mut shop, delete, json!({"id": id}), "priceListDelete");
    assert_eq!(errors[0]["code"], json!("PRICE_LIST_NOT_FOUND"));
}

/// A B2B price list of the rich fixture's first company location: `(list, location)`.
fn b2b_price_list(shop: &mut ShopState) -> (String, String) {
    let location = shop.keys("CompanyLocation").next().unwrap().to_string();
    let answer = run(
        shop,
        "mutation { priceListCreate(input: {name: \"B2B\", currency: EUR, parent: {adjustment: {type: PERCENTAGE_DECREASE, value: 20}}}) { priceList { id } userErrors { message } } }",
        json!({}),
    );
    let list = ok(&answer, "priceListCreate")["priceList"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let answer = run(
        shop,
        CATALOG_CREATE,
        json!({"input": {"title": "B2B", "status": "ACTIVE", "context": {"companyLocationIds": [location]}, "priceListId": list}}),
    );
    ok(&answer, "catalogCreate");
    shop.take_events();
    (list, location)
}

#[test]
fn adds_and_deletes_quantity_rules() {
    let mut shop = rich();
    let (list, _) = b2b_price_list(&mut shop);
    let variants: Vec<String> = shop
        .keys("ProductVariant")
        .take(2)
        .map(str::to_string)
        .collect();
    let add = "mutation($id: ID!, $rules: [QuantityRuleInput!]!) { quantityRulesAdd(priceListId: $id, quantityRules: $rules) { userErrors { field code message } quantityRules { minimum maximum increment isDefault originType productVariant { id } } } }";
    // The reference's example of errors, in its order.
    let errors = refused(
        &mut shop,
        add,
        json!({"id": list, "rules": [
            {"variantId": variants[0], "minimum": 1, "maximum": 5, "increment": 2},
            {"variantId": variants[1], "minimum": 10, "maximum": 5, "increment": 5},
        ]}),
        "quantityRulesAdd",
    );
    assert_eq!(
        errors,
        json!([
            {"field": ["quantityRules", "0", "increment"], "code": "INCREMENT_IS_GREATER_THAN_MINIMUM", "message": "Increment must be lower than or equal to the minimum."},
            {"field": ["quantityRules", "0", "maximum"], "code": "MAXIMUM_NOT_MULTIPLE_OF_INCREMENT", "message": "Maximum must be a multiple of the increment."},
            {"field": ["quantityRules", "0", "minimum"], "code": "MINIMUM_NOT_MULTIPLE_OF_INCREMENT", "message": "Minimum must be a multiple of the increment."},
            {"field": ["quantityRules", "1", "minimum"], "code": "MINIMUM_IS_GREATER_THAN_MAXIMUM", "message": "Minimum must be lower than or equal to the maximum."},
        ])
    );
    let answer = run(
        &mut shop,
        add,
        json!({"id": list, "rules": [
            {"variantId": variants[0], "minimum": 1, "maximum": null, "increment": 1},
            {"variantId": variants[1], "minimum": 5, "maximum": 25, "increment": 5},
        ]}),
    );
    assert_eq!(
        ok(&answer, "quantityRulesAdd")["quantityRules"],
        json!([
            {"minimum": 1, "maximum": null, "increment": 1, "isDefault": true, "originType": "FIXED", "productVariant": {"id": variants[0]}},
            {"minimum": 5, "maximum": 25, "increment": 5, "isDefault": false, "originType": "FIXED", "productVariant": {"id": variants[1]}},
        ]),
        "the reference's example"
    );
    assert_eq!(topics(&mut shop), ["price_lists/update"]);
    let answer = run(
        &mut shop,
        "query($id: ID!) { priceList(id: $id) { quantityRules(first: 5) { nodes { minimum productVariant { id } } } } }",
        json!({"id": list}),
    );
    assert_eq!(
        answer["data"]["priceList"]["quantityRules"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let delete = "mutation($id: ID!, $variants: [ID!]!) { quantityRulesDelete(priceListId: $id, variantIds: $variants) { deletedQuantityRulesVariantIds userErrors { field code message } } }";
    let third = shop.keys("ProductVariant").nth(2).unwrap().to_string();
    let errors = refused(
        &mut shop,
        delete,
        json!({"id": list, "variants": [third]}),
        "quantityRulesDelete",
    );
    assert_eq!(
        errors[0]["code"],
        json!("VARIANT_QUANTITY_RULE_DOES_NOT_EXIST")
    );
    let answer = run(&mut shop, delete, json!({"id": list, "variants": variants}));
    assert_eq!(
        ok(&answer, "quantityRulesDelete")["deletedQuantityRulesVariantIds"],
        json!(variants)
    );
    // A market catalog's price list takes none.
    let answer = run(
        &mut shop,
        "mutation { priceListCreate(input: {name: \"Market\", currency: EUR, parent: {adjustment: {type: PERCENTAGE_DECREASE, value: 0}}}) { priceList { id } userErrors { message } } }",
        json!({}),
    );
    let market_list = ok(&answer, "priceListCreate")["priceList"]["id"].clone();
    let europe = shop.keys("Market").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        CATALOG_CREATE,
        json!({"input": {"title": "Europe", "status": "ACTIVE", "context": {"marketIds": [europe]}, "priceListId": market_list}}),
    );
    ok(&answer, "catalogCreate");
    let errors = refused(
        &mut shop,
        add,
        json!({"id": market_list, "rules": [{"variantId": variants[0], "minimum": 2, "increment": 1}]}),
        "quantityRulesAdd",
    );
    assert_eq!(
        errors[0]["code"],
        json!("CATALOG_CONTEXT_DOES_NOT_SUPPORT_QUANTITY_RULES")
    );
}

const QUANTITY_PRICING: &str = "mutation($id: ID!, $input: QuantityPricingByVariantUpdateInput!, $location: ID!) { quantityPricingByVariantUpdate(priceListId: $id, input: $input) { productVariants { id contextualPricing(context: {companyLocationId: $location}) { price { amount currencyCode } quantityPriceBreaks(first: 1) { nodes { minimumQuantity price { amount currencyCode } } } quantityRule { minimum maximum increment } } } userErrors { message code field } } }";

/// A deleted variant leaves the price lists: its fixed price (`fixedPricesCount` counts again),
/// its quantity rule and its price breaks.
#[test]
fn a_deleted_variant_leaves_the_price_lists() {
    let mut shop = rich();
    let (list, location) = b2b_price_list(&mut shop);
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let product = shop.get(&variant).unwrap()["product"]
        .as_str()
        .unwrap()
        .to_string();
    let answer = run(
        &mut shop,
        QUANTITY_PRICING,
        json!({"id": list, "location": location, "input": {
            "quantityPriceBreaksToAdd": [{"variantId": variant, "price": {"amount": "35", "currencyCode": "EUR"}, "minimumQuantity": 10}],
            "quantityPriceBreaksToDelete": [],
            "quantityRulesToAdd": [{"variantId": variant, "minimum": 10, "maximum": 100, "increment": 5}],
            "quantityRulesToDeleteByVariantId": [],
            "pricesToAdd": [{"price": {"amount": "40", "currencyCode": "EUR"}, "variantId": variant}],
            "pricesToDeleteByVariantId": [],
        }}),
    );
    ok(&answer, "quantityPricingByVariantUpdate");
    let read = |shop: &mut ShopState| {
        run(
            shop,
            "query($id: ID!) { priceList(id: $id) { fixedPricesCount quantityRules(first: 5) { nodes { minimum } } } }",
            json!({"id": list}),
        )["data"]["priceList"]
            .clone()
    };
    assert_eq!(
        read(&mut shop),
        json!({"fixedPricesCount": 1, "quantityRules": {"nodes": [{"minimum": 10}]}})
    );
    assert_eq!(shop.count("QuantityPriceBreak"), 1);
    shop.take_events();
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { productDelete(input: {id: $id}) { deletedProductId userErrors { field message } } }",
        json!({"id": product}),
    );
    ok(&deleted, "productDelete");
    assert_eq!(
        read(&mut shop),
        json!({"fixedPricesCount": 0, "quantityRules": {"nodes": []}})
    );
    assert_eq!(shop.count("QuantityPriceBreak"), 0);
    assert!(
        topics(&mut shop)
            .iter()
            .any(|topic| topic == "price_lists/update")
    );
}

#[test]
fn updates_quantity_pricing_and_prices_a_company_location() {
    let mut shop = rich();
    let (list, location) = b2b_price_list(&mut shop);
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let input = |minimum: i64| {
        json!({
            "quantityPriceBreaksToAdd": [{"variantId": variant, "price": {"amount": "35", "currencyCode": "EUR"}, "minimumQuantity": minimum}],
            "quantityPriceBreaksToDelete": [],
            "quantityRulesToAdd": [{"variantId": variant, "minimum": 10, "maximum": 100, "increment": 5}],
            "quantityRulesToDeleteByVariantId": [],
            "pricesToAdd": [{"price": {"amount": "40", "currencyCode": "EUR"}, "variantId": variant}],
            "pricesToDeleteByVariantId": [],
        })
    };
    // The reference's example of errors, its typo included.
    let errors = refused(
        &mut shop,
        QUANTITY_PRICING,
        json!({"id": list, "input": input(5), "location": location}),
        "quantityPricingByVariantUpdate",
    );
    assert_eq!(
        errors,
        json!([
            {"message": "Miniumum quantity price break must be greater than or equal to the minimum quantity rule.", "code": "QUANTITY_PRICE_BREAK_ADD_MIN_LOWER_THAN_QUANTITY_RULES_MIN", "field": ["input", "quantityPriceBreaksToAdd", "0"]},
            {"message": "Minimum quantity rule must be less than or equal to the minimum quantity price break.", "code": "QUANTITY_RULE_ADD_MIN_HIGHER_THAN_QUANTITY_PRICE_BREAK_MIN", "field": ["input", "quantityRulesToAdd", "0"]},
        ])
    );
    let answer = run(
        &mut shop,
        QUANTITY_PRICING,
        json!({"id": list, "input": input(10), "location": location}),
    );
    assert_eq!(
        ok(&answer, "quantityPricingByVariantUpdate")["productVariants"],
        json!([{"id": variant, "contextualPricing": {
            "price": {"amount": "40.0", "currencyCode": "EUR"},
            "quantityPriceBreaks": {"nodes": [{"minimumQuantity": 10, "price": {"amount": "35.0", "currencyCode": "EUR"}}]},
            "quantityRule": {"minimum": 10, "maximum": 100, "increment": 5},
        }}]),
        "the reference's example"
    );
    assert_eq!(topics(&mut shop), ["price_lists/update"]);
    let price_break = shop.keys("QuantityPriceBreak").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        "query($id: ID!) { node(id: $id) { ... on QuantityPriceBreak { minimumQuantity priceList { id } variant { id } } } }",
        json!({"id": price_break}),
    );
    assert_eq!(answer["data"]["node"]["priceList"]["id"], json!(list));
    // The product's contextual pricing, and another variant's: 20% off the variant's price.
    let other = shop.keys("ProductVariant").nth(1).unwrap().to_string();
    let base = crate::domains::catalog::cents_of(shop.get(&other).unwrap().get("price")).unwrap();
    let answer = run(
        &mut shop,
        "query($id: ID!, $location: ID!) { productVariant(id: $id) { contextualPricing(context: {companyLocationId: $location}) { price { amount } quantityRule { isDefault originType } } } }",
        json!({"id": other, "location": location}),
    );
    assert_eq!(
        answer["data"]["productVariant"]["contextualPricing"]["price"]["amount"],
        json!(crate::state::value::decimal(base * 8 / 10))
    );
    assert_eq!(
        answer["data"]["productVariant"]["contextualPricing"]["quantityRule"],
        json!({"isDefault": true, "originType": "RELATIVE"})
    );
    let product = shop.get(&variant).unwrap()["product"]
        .as_str()
        .unwrap()
        .to_string();
    let answer = run(
        &mut shop,
        "query($id: ID!, $location: ID!) { product(id: $id) { contextualPricing(context: {companyLocationId: $location}) { fixedQuantityRulesCount priceRange { minVariantPrice { amount } } } } }",
        json!({"id": product, "location": location}),
    );
    assert_eq!(
        answer["data"]["product"]["contextualPricing"]["fixedQuantityRulesCount"],
        json!(1)
    );
    // Deleting, all of it.
    let answer = run(
        &mut shop,
        QUANTITY_PRICING,
        json!({"id": list, "location": location, "input": {
            "quantityPriceBreaksToAdd": [], "quantityPriceBreaksToDelete": [price_break],
            "quantityRulesToAdd": [], "quantityRulesToDeleteByVariantId": [variant],
            "pricesToAdd": [], "pricesToDeleteByVariantId": [variant],
        }}),
    );
    ok(&answer, "quantityPricingByVariantUpdate");
    assert_eq!(shop.count("QuantityPriceBreak"), 0);
    // A price break without a fixed price.
    let errors = refused(
        &mut shop,
        QUANTITY_PRICING,
        json!({"id": list, "location": location, "input": {
            "quantityPriceBreaksToAdd": [{"variantId": variant, "price": {"amount": "1", "currencyCode": "EUR"}, "minimumQuantity": 3}],
            "quantityPriceBreaksToDelete": [], "quantityRulesToAdd": [], "quantityRulesToDeleteByVariantId": [],
            "pricesToAdd": [], "pricesToDeleteByVariantId": [],
        }}),
        "quantityPricingByVariantUpdate",
    );
    assert_eq!(
        errors[0]["code"],
        json!("QUANTITY_PRICE_BREAK_ADD_PRICE_LIST_PRICE_NOT_FOUND")
    );
}

#[test]
fn prices_a_country_from_its_market_catalog() {
    let mut shop = rich();
    let europe = shop.keys("Market").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        "mutation { priceListCreate(input: {name: \"Europe\", currency: EUR, parent: {adjustment: {type: PERCENTAGE_INCREASE, value: 50}}}) { priceList { id } userErrors { message } } }",
        json!({}),
    );
    let list = ok(&answer, "priceListCreate")["priceList"]["id"].clone();
    let answer = run(
        &mut shop,
        CATALOG_CREATE,
        json!({"input": {"title": "Europe", "status": "ACTIVE", "context": {"marketIds": [europe]}, "priceListId": list}}),
    );
    ok(&answer, "catalogCreate");
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let base = crate::domains::catalog::cents_of(shop.get(&variant).unwrap().get("price")).unwrap();
    let query = "query($id: ID!, $country: CountryCode!) { productVariant(id: $id) { contextualPricing(context: {country: $country}) { price { amount currencyCode } } } }";
    let answer = run(&mut shop, query, json!({"id": variant, "country": "FR"}));
    assert_eq!(
        answer["data"]["productVariant"]["contextualPricing"]["price"],
        json!({"amount": crate::state::value::decimal(base * 15 / 10), "currencyCode": "EUR"})
    );
    // A country of no catalog: the variant's own price, in its market's currency (GBP, at a rate
    // lsf does not have).
    let answer = run(&mut shop, query, json!({"id": variant, "country": "GB"}));
    assert_eq!(
        answer["data"]["productVariant"]["contextualPricing"]["price"],
        json!({"amount": crate::state::value::decimal(base), "currencyCode": "GBP"})
    );
    assert!(
        answer["extensions"]["lsf"]["notes"][0]
            .as_str()
            .unwrap()
            .contains("exchange rates")
    );
}

#[test]
fn creates_updates_and_deletes_publications() {
    let mut shop = rich();
    shop.take_events();
    let europe = shop.keys("Market").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        CATALOG_CREATE,
        json!({"input": {"title": "Europe", "status": "ACTIVE", "context": {"marketIds": [europe]}}}),
    );
    let catalog = ok(&answer, "catalogCreate")["catalog"]["id"].clone();
    shop.take_events();
    let create = "mutation($input: PublicationCreateInput!) { publicationCreate(input: $input) { publication { id name autoPublish catalog { id } products(first: 50) { nodes { id } } } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        create,
        json!({"input": {"catalogId": catalog, "defaultState": "ALL_PRODUCTS", "autoPublish": true}}),
    );
    let publication = ok(&answer, "publicationCreate")["publication"].clone();
    assert_eq!(publication["name"], json!("Europe"));
    assert_eq!(publication["catalog"]["id"], catalog);
    assert_eq!(
        publication["products"]["nodes"].as_array().unwrap().len(),
        shop.count("Product")
    );
    let id = publication["id"].as_str().unwrap().to_string();
    let emitted = topics(&mut shop);
    assert_eq!(emitted[0], "publications/create");
    assert!(emitted.contains(&"product_publications/create".to_string()));
    let errors = refused(
        &mut shop,
        create,
        json!({"input": {"catalogId": catalog}}),
        "publicationCreate",
    );
    assert_eq!(errors[0]["code"], json!("TAKEN"));
    let errors = refused(
        &mut shop,
        create,
        json!({"input": {"catalogId": super::online_store_catalog_gid()}}),
        "publicationCreate",
    );
    assert_eq!(errors[0]["code"], json!("CANNOT_MODIFY_APP_CATALOG"));
    // A product created joins it: `autoPublish`.
    let answer = run(
        &mut shop,
        "mutation { productCreate(product: {title: \"Scarf\"}) { product { id } userErrors { message } } }",
        json!({}),
    );
    let scarf = ok(&answer, "productCreate")["product"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let answer = run(
        &mut shop,
        "query($id: ID!, $product: ID!) { publication(id: $id) { products(first: 50) { nodes { id } } } product(id: $product) { publishedOnPublication(publicationId: $id) } }",
        json!({"id": id, "product": scarf}),
    );
    assert_eq!(
        answer["data"]["product"]["publishedOnPublication"],
        json!(true)
    );
    shop.take_events();
    // Update: a product removed, a collection added.
    let collection = shop.keys("Collection").next().unwrap().to_string();
    let update = "mutation($id: ID!, $input: PublicationUpdateInput!) { publicationUpdate(id: $id, input: $input) { publication { autoPublish } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        update,
        json!({"id": id, "input": {"autoPublish": false, "publishablesToAdd": [collection], "publishablesToRemove": [scarf]}}),
    );
    assert_eq!(
        ok(&answer, "publicationUpdate")["publication"]["autoPublish"],
        json!(false)
    );
    let emitted = topics(&mut shop);
    assert!(
        emitted.contains(&"collection_publications/create".to_string()),
        "{emitted:?}"
    );
    assert!(
        emitted.contains(&"product_publications/delete".to_string()),
        "{emitted:?}"
    );
    let many: Vec<String> = (0..51).map(|_| scarf.clone()).collect();
    let errors = refused(
        &mut shop,
        update,
        json!({"id": id, "input": {"publishablesToAdd": many}}),
        "publicationUpdate",
    );
    assert_eq!(
        errors[0]["code"],
        json!("PUBLICATION_UPDATE_LIMIT_EXCEEDED")
    );
    let online_store = crate::domains::catalog::online_store_publication_gid();
    let errors = refused(
        &mut shop,
        update,
        json!({"id": online_store, "input": {"autoPublish": true}}),
        "publicationUpdate",
    );
    assert_eq!(
        errors[0]["code"],
        json!("CANNOT_MODIFY_APP_CATALOG_PUBLICATION")
    );
    let answer = run(
        &mut shop,
        "{ publications(first: 5, catalogType: MARKET) { nodes { id } } publicationsCount { count } shop { publicationCount } }",
        json!({}),
    );
    assert_eq!(answer["data"]["publications"]["nodes"], json!([{"id": id}]));
    assert_eq!(answer["data"]["publicationsCount"]["count"], json!(2));
    assert_eq!(answer["data"]["shop"]["publicationCount"], json!(2));
    // Delete.
    let delete = "mutation($id: ID!) { publicationDelete(id: $id) { deletedId userErrors { field message code } } }";
    refused(
        &mut shop,
        delete,
        json!({"id": online_store}),
        "publicationDelete",
    );
    let answer = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(ok(&answer, "publicationDelete")["deletedId"], json!(id));
    assert_eq!(
        shop.get(catalog.as_str().unwrap()).unwrap()["publication"],
        Value::Null
    );
}

const CHANNEL_CREATE: &str = "mutation($input: ChannelCreateInput!) { channelCreate(input: $input) { channel { id name handle accountId accountName specificationHandle activeRegions app { id } supportsFuturePublishing markets(first: 5) { nodes { name } } } userErrors { field message code } } }";

/// A channel of the app: its GID.
fn channel(shop: &mut ShopState) -> String {
    let answer = run(
        shop,
        CHANNEL_CREATE,
        json!({"input": {"accountId": "A1B2C3", "accountName": "Example marketplace", "specificationHandle": "example-us"}}),
    );
    let id = ok(&answer, "channelCreate")["channel"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    shop.take_events();
    id
}

#[test]
fn creates_updates_syncs_and_deletes_channels() {
    let mut shop = demo();
    shop.take_events();
    let answer = run(
        &mut shop,
        CHANNEL_CREATE,
        json!({"input": {"accountId": "A1B2C3", "accountName": "Example marketplace", "specificationHandle": "example-us"}}),
    );
    let created = ok(&answer, "channelCreate")["channel"].clone();
    assert_eq!(
        created["handle"],
        json!("example-us-a1b2c3"),
        "from the specification and account"
    );
    assert_eq!(created["activeRegions"], json!(["US", "CA", "FR"]));
    assert_eq!(created["supportsFuturePublishing"], json!(true));
    assert_eq!(created["markets"]["nodes"].as_array().unwrap().len(), 3);
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["channels/create"]);
    let errors = refused(
        &mut shop,
        CHANNEL_CREATE,
        json!({"input": {"accountId": "A1B2C3", "accountName": "Again", "specificationHandle": "example-us"}}),
        "channelCreate",
    );
    assert_eq!(errors[0]["code"], json!("INVALID"));
    refused(
        &mut shop,
        CHANNEL_CREATE,
        json!({"input": {"accountId": "", "accountName": "X", "specificationHandle": "s"}}),
        "channelCreate",
    );
    // Its publication and catalog.
    let answer = run(
        &mut shop,
        "query($id: ID!) { channel(id: $id) { handle } channels(first: 5) { nodes { id } } channelByHandle(handle: \"example-us-a1b2c3\") { id } catalogs(first: 5, type: APP) { nodes { title } } publications(first: 5, catalogType: APP) { nodes { name supportsFuturePublishing channels(first: 1) { nodes { id } } } } }",
        json!({"id": id}),
    );
    assert!(answer.get("errors").is_none(), "{answer}");
    assert_eq!(answer["data"]["channelByHandle"]["id"], json!(id));
    assert_eq!(
        answer["data"]["catalogs"]["nodes"],
        json!([{"title": "Online Store"}, {"title": "Example marketplace"}])
    );
    assert_eq!(
        answer["data"]["publications"]["nodes"][1]["channels"]["nodes"],
        json!([{"id": id}])
    );
    // Update.
    let update = "mutation($id: ID!, $input: ChannelUpdateInput!) { channelUpdate(id: $id, input: $input) { channel { name accountName handle } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        update,
        json!({"id": id, "input": {"accountName": "Renamed", "handle": "renamed"}}),
    );
    assert_eq!(
        ok(&answer, "channelUpdate")["channel"],
        json!({"name": "Renamed", "accountName": "Renamed", "handle": "renamed"})
    );
    refused(
        &mut shop,
        update,
        json!({"id": "gid://shopify/Channel/999999", "input": {"accountName": "X"}}),
        "channelUpdate",
    );
    // Full sync: a trace per country and published language.
    let sync = "mutation($id: ID!, $country: CountryCode) { channelFullSync(channelId: $id, country: $country) { fullSyncTraceInfo { country language operationId } userErrors { field message code } } }";
    let answer = run(&mut shop, sync, json!({"id": id, "country": "FR"}));
    let traces = ok(&answer, "channelFullSync")["fullSyncTraceInfo"].clone();
    assert_eq!(traces.as_array().unwrap().len(), 2);
    assert_eq!(traces[0]["country"], json!("FR"));
    assert_eq!(traces[1]["language"], json!("FR"));
    assert!(
        traces[0]["operationId"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/ProductFullSync/")
    );
    let errors = refused(
        &mut shop,
        sync,
        json!({"id": id, "country": "JP"}),
        "channelFullSync",
    );
    assert_eq!(errors[0]["field"], json!(["country"]));
    // Delete: `channels/delete`, its publication and catalog gone.
    shop.take_events();
    let delete = "mutation($id: ID!) { channelDelete(id: $id) { deletedId userErrors { field message code } } }";
    let answer = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(ok(&answer, "channelDelete")["deletedId"], json!(id));
    assert_eq!(shop.count("Channel"), 0);
    assert_eq!(shop.count("Publication"), 1);
    let emitted = shop.take_events();
    assert_eq!(emitted.last().unwrap().topic, "channels/delete");
    refused(&mut shop, delete, json!({"id": id}), "channelDelete");
}

#[test]
fn listings_follow_what_is_published_on_a_channel() {
    let mut shop = demo();
    let id = channel(&mut shop);
    let publication = shop.get(&id).unwrap()["lsfPublication"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        shop.get(&publication).unwrap()["lsfChannel"],
        json!(id),
        "the channel's publication names it"
    );
    let (product, _) = first_variant(&shop);
    // Publishing on the current channel: `product_listings/add`.
    let publish = "mutation($id: ID!) { publishablePublishToCurrentChannel(id: $id) { publishable { ... on Product { publishedOnCurrentChannel } } shop { publicationCount } userErrors { field message } } }";
    let answer = run(&mut shop, publish, json!({"id": product}));
    let payload = ok(&answer, "publishablePublishToCurrentChannel");
    assert_eq!(
        payload["publishable"]["publishedOnCurrentChannel"],
        json!(true)
    );
    assert_eq!(payload["shop"]["publicationCount"], json!(2));
    let emitted = shop.take_events();
    let listing = emitted
        .iter()
        .find(|event| event.topic == "product_listings/add")
        .expect("product_listings/add");
    let built = crate::webhooks::payload::payload(
        "product_listings/add",
        listing.snapshot.as_ref().unwrap(),
        &crate::api::samples::ShopLookup(&shop),
    )
    .unwrap();
    assert_eq!(
        built["product_listing"]["product_id"],
        json!(crate::state::parse_gid(&product).unwrap().1)
    );
    assert_eq!(
        built["product_listing"]["published_at"],
        json!("2026-10-31T12:00:00Z")
    );
    let errors = refused(
        &mut shop,
        publish,
        json!({"id": "gid://shopify/Product/1"}),
        "publishablePublishToCurrentChannel",
    );
    assert_eq!(
        errors,
        json!([{"field": ["id"], "message": "Resource does not exist"}])
    );
    // Changed while listed: `product_listings/update`.
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { productUpdate(product: {id: $id, title: \"Renamed\"}) { userErrors { message } } }",
        json!({"id": product}),
    );
    ok(&answer, "productUpdate");
    let emitted = topics(&mut shop);
    assert!(
        emitted.contains(&"product_listings/update".to_string()),
        "{emitted:?}"
    );
    // Unpublished: `product_listings/remove`.
    let unpublish = "mutation($id: ID!) { publishableUnpublishToCurrentChannel(id: $id) { publishable { ... on Product { publishedOnCurrentChannel } } userErrors { field message } } }";
    let answer = run(&mut shop, unpublish, json!({"id": product}));
    assert_eq!(
        ok(&answer, "publishableUnpublishToCurrentChannel")["publishable"]["publishedOnCurrentChannel"],
        json!(false)
    );
    let emitted = topics(&mut shop);
    assert!(
        emitted.contains(&"product_listings/remove".to_string()),
        "{emitted:?}"
    );
    refused(
        &mut shop,
        unpublish,
        json!({"id": "gid://shopify/Collection/1"}),
        "publishableUnpublishToCurrentChannel",
    );
    // A collection.
    let collection = shop.keys("Collection").next().unwrap().to_string();
    let answer = run(&mut shop, publish, json!({"id": collection}));
    ok(&answer, "publishablePublishToCurrentChannel");
    let emitted = topics(&mut shop);
    assert!(
        emitted.contains(&"collection_listings/add".to_string()),
        "{emitted:?}"
    );
    let answer = run(
        &mut shop,
        "query($id: ID!, $collection: ID!) { channel(id: $id) { collections(first: 5) { nodes { id } } hasCollection(id: $collection) } }",
        json!({"id": id, "collection": collection}),
    );
    assert_eq!(answer["data"]["channel"]["hasCollection"], json!(true));
    // Deleting a listed product: `product_listings/remove`.
    let answer = run(&mut shop, publish, json!({"id": product}));
    ok(&answer, "publishablePublishToCurrentChannel");
    shop.take_events();
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { productDelete(input: {id: $id}) { deletedProductId userErrors { message } } }",
        json!({"id": product}),
    );
    ok(&answer, "productDelete");
    let emitted = topics(&mut shop);
    assert!(
        emitted.contains(&"product_listings/remove".to_string()),
        "{emitted:?}"
    );
}

#[test]
fn a_future_publish_date_on_a_channel_schedules_the_listing() {
    let mut shop = demo();
    let channel = channel(&mut shop);
    let publication = shop.get(&channel).unwrap()["lsfPublication"]
        .as_str()
        .unwrap()
        .to_string();
    let (product, _) = first_variant(&shop);
    let publish = "mutation($id: ID!, $publication: ID!, $date: DateTime) { publishablePublish(id: $id, input: [{publicationId: $publication, publishDate: $date}]) { userErrors { field message } } }";
    let answer = run(
        &mut shop,
        publish,
        json!({"id": product, "publication": publication, "date": "2026-11-02T09:00:00Z"}),
    );
    ok(&answer, "publishablePublish");
    assert!(
        answer.get("extensions").is_none(),
        "scheduled, not published at once: {answer}"
    );
    let emitted = shop.take_events();
    let scheduled = emitted
        .iter()
        .find(|event| event.topic == "scheduled_product_listings/add")
        .expect("scheduled_product_listings/add");
    let built = crate::webhooks::payload::payload(
        "scheduled_product_listings/add",
        scheduled.snapshot.as_ref().unwrap(),
        &crate::api::samples::ShopLookup(&shop),
    )
    .unwrap();
    assert_eq!(
        built["scheduled_product_listing"]["publish_at"],
        json!("2026-11-02T09:00:00Z")
    );
    assert!(
        !emitted
            .iter()
            .any(|event| event.topic == "product_publications/create")
    );
    let answer = run(
        &mut shop,
        "query($id: ID!, $channel: ID!) { product(id: $id) { publishedOnChannel(channelId: $channel) } }",
        json!({"id": product, "channel": channel}),
    );
    assert_eq!(
        answer["data"]["product"]["publishedOnChannel"],
        json!(false)
    );
    // Once the date has passed, the next mutation lists it.
    shop.set_clock(Clock::Frozen(at("2026-11-03T00:00:00Z")));
    let answer = run(
        &mut shop,
        "mutation { shopLocaleEnable(locale: \"de\") { userErrors { message } } }",
        json!({}),
    );
    ok(&answer, "shopLocaleEnable");
    let emitted = topics(&mut shop);
    assert!(
        emitted.contains(&"product_listings/add".to_string()),
        "{emitted:?}"
    );
    let answer = run(
        &mut shop,
        "query($id: ID!, $channel: ID!) { product(id: $id) { publishedOnChannel(channelId: $channel) } }",
        json!({"id": product, "channel": channel}),
    );
    assert_eq!(answer["data"]["product"]["publishedOnChannel"], json!(true));
    // Scheduled again, then unpublished before the date: `scheduled_product_listings/remove`.
    let other = shop.keys("Product").nth(1).unwrap().to_string();
    let answer = run(
        &mut shop,
        publish,
        json!({"id": other, "publication": publication, "date": "2026-12-01T00:00:00Z"}),
    );
    ok(&answer, "publishablePublish");
    shop.take_events();
    let answer = run(
        &mut shop,
        "mutation($id: ID!, $publication: ID!) { publishableUnpublish(id: $id, input: [{publicationId: $publication}]) { userErrors { message } } }",
        json!({"id": other, "publication": publication}),
    );
    ok(&answer, "publishableUnpublish");
    assert_eq!(topics(&mut shop), ["scheduled_product_listings/remove"]);
}

#[test]
fn records_product_resource_feedback() {
    let mut shop = demo();
    let (product, _) = first_variant(&shop);
    let create = "mutation($input: [ProductResourceFeedbackInput!]!) { bulkProductResourceFeedbackCreate(feedbackInput: $input) { feedback { productId state messages feedbackGeneratedAt productUpdatedAt } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        create,
        json!({"input": [{"productId": product, "state": "REQUIRES_ACTION", "feedbackGeneratedAt": "2026-10-30T23:00:00Z", "productUpdatedAt": "2026-10-28T16:00:00Z", "messages": ["Needs a description."]}]}),
    );
    assert_eq!(
        ok(&answer, "bulkProductResourceFeedbackCreate")["feedback"][0]["messages"],
        json!(["Needs a description."])
    );
    let answer = run(
        &mut shop,
        "query($id: ID!) { productResourceFeedback(id: $id) { productId state messages } }",
        json!({"id": product}),
    );
    assert_eq!(
        answer["data"]["productResourceFeedback"],
        json!({"productId": product, "state": "REQUIRES_ACTION", "messages": ["Needs a description."]})
    );
    // Older than the one accepted: the reference's example error.
    let errors = refused(
        &mut shop,
        create,
        json!({"input": [{"productId": product, "state": "REQUIRES_ACTION", "feedbackGeneratedAt": "2026-10-29T23:00:00Z", "productUpdatedAt": "2026-10-28T16:00:00Z", "messages": ["x"]}]}),
        "bulkProductResourceFeedbackCreate",
    );
    assert_eq!(
        errors,
        json!([{"field": ["feedbackInput", "0", "feedbackGeneratedAt"], "message": "Feedback for a later version of this resource was already accepted.", "code": "OUTDATED_FEEDBACK"}])
    );
    let errors = refused(
        &mut shop,
        create,
        json!({"input": [{"productId": "gid://shopify/Product/1", "state": "ACCEPTED", "feedbackGeneratedAt": "2026-10-30T23:00:00Z", "productUpdatedAt": "2026-10-28T16:00:00Z"}]}),
        "bulkProductResourceFeedbackCreate",
    );
    assert_eq!(errors[0]["code"], json!("PRODUCT_NOT_FOUND"));
}

#[test]
fn records_the_app_s_feedback_on_the_shop_and_its_channels() {
    let mut shop = demo();
    let channel = channel(&mut shop);
    let create = "mutation($input: ResourceFeedbackCreateInput!) { shopResourceFeedbackCreate(input: $input) { feedback { messages { message } feedbackGeneratedAt state } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        create,
        json!({"input": {"channelId": channel, "messages": ["is not connected. Connect your account to use this sales channel."], "state": "REQUIRES_ACTION", "feedbackGeneratedAt": "2026-10-30T14:05:02.351662Z"}}),
    );
    assert_eq!(
        ok(&answer, "shopResourceFeedbackCreate")["feedback"],
        json!({"messages": [{"message": "Example marketplace is not connected. Connect your account to use this sales channel."}], "feedbackGeneratedAt": "2026-10-30T14:05:02Z", "state": "REQUIRES_ACTION"}),
        "the name first, the date to the second: the reference's example"
    );
    let answer = run(
        &mut shop,
        "query($id: ID!) { channel(id: $id) { resourceFeedback { state } } }",
        json!({"id": channel}),
    );
    assert_eq!(
        answer["data"]["channel"]["resourceFeedback"]["state"],
        json!("REQUIRES_ACTION")
    );
    let errors = refused(
        &mut shop,
        create,
        json!({"input": {"channelId": channel, "messages": ["x"], "state": "REQUIRES_ACTION", "feedbackGeneratedAt": "1998-06-07T10:00:00Z"}}),
        "shopResourceFeedbackCreate",
    );
    assert_eq!(
        errors,
        json!([{"field": ["input"], "message": "Feedback for a later version of this resource was already accepted.", "code": "OUTDATED_FEEDBACK"}])
    );
    let answer = run(
        &mut shop,
        create,
        json!({"input": {"channelId": channel, "state": "ACCEPTED", "feedbackGeneratedAt": "2026-10-31T00:00:00Z"}}),
    );
    assert_eq!(
        ok(&answer, "shopResourceFeedbackCreate")["feedback"],
        Value::Null,
        "cleared"
    );
    let answer = run(&mut shop, "{ app { feedback { state } } }", json!({}));
    assert_eq!(answer["data"]["app"]["feedback"], Value::Null);
}

#[test]
fn the_records_this_family_makes_are_in_the_shape_of_their_types() {
    // `tests/shape.rs` checks the seed; the integration tests (`tests/markets.rs`) what the
    // mutations make. Here: that every mutation of the family is registered.
    let registry = Registry::standard();
    for name in [
        "backupRegionUpdate",
        "bulkProductResourceFeedbackCreate",
        "catalogContextUpdate",
        "catalogCreate",
        "catalogDelete",
        "catalogUpdate",
        "channelCreate",
        "channelDelete",
        "channelFullSync",
        "channelUpdate",
        "marketCreate",
        "marketCurrencySettingsUpdate",
        "marketDelete",
        "marketLocalizationsRegister",
        "marketLocalizationsRemove",
        "marketRegionDelete",
        "marketRegionsCreate",
        "marketRegionsDelete",
        "marketUpdate",
        "marketWebPresenceCreate",
        "marketWebPresenceDelete",
        "marketWebPresenceUpdate",
        "priceListCreate",
        "priceListDelete",
        "priceListFixedPricesAdd",
        "priceListFixedPricesByProductUpdate",
        "priceListFixedPricesDelete",
        "priceListFixedPricesUpdate",
        "priceListUpdate",
        "publicationCreate",
        "publicationDelete",
        "publicationUpdate",
        "publishablePublishToCurrentChannel",
        "publishableUnpublishToCurrentChannel",
        "quantityPricingByVariantUpdate",
        "quantityRulesAdd",
        "quantityRulesDelete",
        "shopLocaleDisable",
        "shopLocaleEnable",
        "shopLocaleUpdate",
        "shopResourceFeedbackCreate",
        "webPresenceCreate",
        "webPresenceDelete",
        "webPresenceUpdate",
    ] {
        assert!(
            registry.mutation_fn(name).is_some(),
            "{name} is not simulated"
        );
    }
}

#[test]
fn what_the_mutations_make_answers_without_defaults() {
    let mut shop = rich();
    let (list, location) = b2b_price_list(&mut shop);
    let _ = channel(&mut shop);
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        QUANTITY_PRICING,
        json!({"id": list, "location": location, "input": {
        "quantityPriceBreaksToAdd": [{"variantId": variant, "price": {"amount": "35", "currencyCode": "EUR"}, "minimumQuantity": 10}],
        "quantityPriceBreaksToDelete": [], "quantityRulesToAdd": [], "quantityRulesToDeleteByVariantId": [],
        "pricesToAdd": [{"price": {"amount": "40", "currencyCode": "EUR"}, "variantId": variant}], "pricesToDeleteByVariantId": []}}),
    );
    ok(&answer, "quantityPricingByVariantUpdate");
    let answer = run(
        &mut shop,
        r#"{
      priceLists(first: 5) { nodes { id name currency fixedPricesCount createdAt parent { adjustment { type value } settings { compareAtMode } } catalog { id title status }
        prices(first: 2) { nodes { originType price { amount } compareAtPrice { amount } variant { id } quantityPriceBreaks(first: 2) { nodes { id minimumQuantity price { amount } } } } }
        quantityRules(first: 2) { nodes { minimum maximum increment isDefault originType productVariant { id } } } } }
      catalogs(first: 5) { nodes { id title status operations { id } priceList { id } publication { id }
        ... on CompanyLocationCatalog { companyLocations(first: 2) { nodes { id } } companyLocationsCount { count } }
        ... on AppCatalog { apps(first: 2) { nodes { id } } channels(first: 2) { nodes { id } } } } }
      channels(first: 5) { nodes { id name handle accountId accountName specificationHandle activeRegions app { id } supportsFuturePublishing navigationItems { id } overviewPath resourceFeedback { state }
        products(first: 2) { nodes { id } } productsCount { count } collections(first: 2) { nodes { id } } productPublicationsV3(first: 2) { nodes { isPublished publishDate } } markets(first: 2) { nodes { id } } marketsCount { count } } }
      publications(first: 5) { nodes { id name autoPublish supportsFuturePublishing operation { __typename } catalog { id } channels(first: 2) { nodes { id } } } }
      webPresences(first: 5) { nodes { id domain { id } subfolderSuffix rootUrls { url } market { id } } }
    }"#,
        json!({}),
    );
    assert!(answer.get("errors").is_none(), "{answer}");
    assert!(
        answer.get("extensions").is_none(),
        "defaulted: {}",
        answer["extensions"]
    );
}
