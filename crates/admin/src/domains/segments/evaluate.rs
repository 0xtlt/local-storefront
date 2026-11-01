//! Evaluates a segment query on the customers of a shop: [`Facts`] is what the filters read of a
//! customer (computed once per customer and request), [`matches`] whether a [`Query`] keeps them.
//!
//! What each filter reads, and what lsf cannot know:
//!
//! - `amount_spent`, `number_of_orders`, `customer_added_date`, `customer_account_status`,
//!   `customer_tags` (case ignored, as the reference says of lists), `customer_language` (`en`
//!   also matches `en-GB`, as the Help Center says), `customer_email_domain`, the email and SMS
//!   consents: the customer's record.
//! - `customer_countries` (`'US'`), `customer_regions` (`'US-NY'`), `customer_cities`
//!   (`'US-NY-NewYorkCity'`: the country, the province code and the city's words run together,
//!   the Help Center's format): the customer's addresses.
//! - `first_order_date`, `last_order_date`, `orders_placed` (`count`, `amount`, `sum_amount`,
//!   `date`, `app_id`, `location_id`), `products_purchased` (`id`, `tag`, `quantity`,
//!   `sum_quantity`, `date`): the customer's orders and their line items. An order's app is the
//!   Online Store's (`580111`, the `app_id` Shopify gives web orders) unless the record names
//!   another *(chosen)*.
//! - `companies`: the companies the customer is a contact of. `created_by_app_id`: the app, for
//!   a customer the Admin API created; nothing for those of the store data.
//! - `abandoned_checkout_date`: the customer's last abandoned checkout (`domains::checkouts`).
//! - `store_credit_accounts`: the shop's `StoreCreditAccount` records of the customer.
//! - `customer_within_distance`: the coordinates of the customer's addresses, when they have
//!   some (the store data gives none: no customer is within any distance).
//! - `anniversary('metafields.<namespace>.<key>')`: a date metafield of the customer, its day
//!   and month compared with the condition's, whatever the year.
//! - Not known to lsf, so `IS NULL` (or never matched): `predicted_spend_tier` and `rfm_group`
//!   (Shopify's models), `shopify_email.*` (lsf sends no email), `storefront_event.*` (the
//!   storefront records no views). `product_subscription_status` reads the customer's
//!   `productSubscriberStatus` when a record sets it, `NEVER_SUBSCRIBED` otherwise.
//!
//! Dates are whole days in the shop's time zone, as the reference says: `last_order_date =
//! 2020-05-30` is that day from midnight to midnight, `> 2020-05-30` from the next midnight, `-7d`
//! seven days before today, `BETWEEN` both days included. A date-time is a time in the shop's
//! time zone.

use std::collections::HashSet;

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::Value as Json;

use super::language::{Clause, DateValue, Expr, Op, Param, ParamTest, Query, Test, Unit, Value};
use crate::domains::catalog::tags_of;
use crate::state::{Record, ShopState, parse_gid, value};

/// The `app_id` of the orders of the Online Store, which Shopify writes on web orders.
const ONLINE_STORE_APP_ID: u64 = 580111;

/// Where a query is evaluated: the clock and the time zone dates are read in.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub now: DateTime<Utc>,
    pub zone: Tz,
}

impl Context {
    /// The shop's clock, in the shop's time zone (`Shop.ianaTimezone`, UTC without one).
    pub fn of(state: &ShopState) -> Context {
        Context {
            now: state.now(),
            zone: state.time_zone(),
        }
    }

    fn today(&self) -> NaiveDate {
        self.now.with_timezone(&self.zone).date_naive()
    }

    /// The first instant of a day in the zone.
    fn start_of(&self, day: NaiveDate) -> DateTime<Utc> {
        let midnight = day.and_hms_opt(0, 0, 0).unwrap_or_default();
        self.zone
            .from_local_datetime(&midnight)
            .earliest()
            .map_or_else(
                || Utc.from_utc_datetime(&midnight),
                |at| at.with_timezone(&Utc),
            )
    }

    /// The day a date condition names, or the instant of a date-time; `None` past the dates
    /// chrono holds (the parser keeps offsets well within them: [`super::language::
    /// MAX_OFFSET_YEARS`]).
    fn bounds(&self, date: &DateValue) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let day = |day: NaiveDate| Some((self.start_of(day), self.start_of(day.succ_opt()?)));
        match date {
            DateValue::Day(d) => day(*d),
            DateValue::Today => day(self.today()),
            DateValue::Yesterday => day(self.today().pred_opt()?),
            DateValue::Offset(n, unit) => day(offset(self.today(), *n, *unit)?),
            DateValue::Time(time) => {
                let at = self
                    .zone
                    .from_local_datetime(time)
                    .earliest()
                    .map_or_else(|| Utc.from_utc_datetime(time), |at| at.with_timezone(&Utc));
                Some((at, at.checked_add_signed(Duration::seconds(1))?))
            }
        }
    }
}

/// A day moved by `n` units; `None` past the dates chrono holds.
fn offset(day: NaiveDate, n: i64, unit: Unit) -> Option<NaiveDate> {
    match unit {
        Unit::Day => day.checked_add_signed(Duration::try_days(n)?),
        Unit::Week => day.checked_add_signed(Duration::try_weeks(n)?),
        Unit::Month | Unit::Year => {
            let months = if unit == Unit::Year {
                n.checked_mul(12)?
            } else {
                n
            };
            let count = chrono::Months::new(u32::try_from(months.unsigned_abs()).ok()?);
            if months >= 0 {
                day.checked_add_months(count)
            } else {
                day.checked_sub_months(count)
            }
        }
    }
}

/// Whether an instant compares to a date condition as `op` asks: on the day, after its end,
/// before its start, … A date past what chrono holds matches nothing.
fn compare_date(context: &Context, at: DateTime<Utc>, op: Op, date: &DateValue) -> bool {
    let Some((start, end)) = context.bounds(date) else {
        return false;
    };
    match op {
        Op::Eq => start <= at && at < end,
        Op::Ne => !(start <= at && at < end),
        Op::Gt => at >= end,
        Op::Ge => at >= start,
        Op::Lt => at < start,
        Op::Le => at < end,
    }
}

fn between_dates(context: &Context, at: DateTime<Utc>, from: &DateValue, to: &DateValue) -> bool {
    let (Some((start, _)), Some((_, end))) = (context.bounds(from), context.bounds(to)) else {
        return false;
    };
    start <= at && at < end
}

/// An order of a customer, as the filters read it.
#[derive(Clone, Debug, Default)]
pub struct OrderFacts {
    pub at: Option<DateTime<Utc>>,
    /// In the shop's currency.
    pub total: f64,
    pub app_id: u64,
    pub location_id: Option<u64>,
    /// `(product id, product tags, quantity)`.
    pub lines: Vec<(Option<u64>, Vec<String>, i64)>,
}

/// A store credit account of a customer.
#[derive(Clone, Debug, Default)]
pub struct CreditFacts {
    pub balance: f64,
    pub currency: String,
    pub next_expiry: Option<DateTime<Utc>>,
    pub last_credit: Option<DateTime<Utc>>,
}

/// What the filters read of one customer.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub id: String,
    pub amount_spent: f64,
    pub number_of_orders: i64,
    pub created_at: Option<DateTime<Utc>>,
    pub account_status: String,
    pub email_domain: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub email_state: Option<String>,
    pub sms_state: Option<String>,
    pub countries: Vec<String>,
    pub regions: Vec<String>,
    pub cities: Vec<String>,
    pub coordinates: Vec<(f64, f64)>,
    pub companies: Vec<u64>,
    pub created_by_app: Option<u64>,
    pub abandoned_checkout: Option<DateTime<Utc>>,
    pub product_subscription: String,
    pub orders: Vec<OrderFacts>,
    pub credits: Vec<CreditFacts>,
    /// The customer's metafields, `namespace.key` → value, for `anniversary`.
    pub metafields: Vec<(String, String)>,
}

impl Facts {
    pub fn first_order(&self) -> Option<DateTime<Utc>> {
        self.orders.iter().filter_map(|order| order.at).min()
    }

    pub fn last_order(&self) -> Option<DateTime<Utc>> {
        self.orders.iter().filter_map(|order| order.at).max()
    }
}

fn decimal(value: Option<&Json>) -> f64 {
    value::exact_cents(value).map_or(0.0, |cents| cents as f64 / 100.0)
}

/// The city of an address as the Help Center writes it in `customer_cities`: its words run
/// together, each with a capital (`New York City` is `NewYorkCity`).
fn city_code(city: &str) -> String {
    city.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect::<String>()
            })
        })
        .collect()
}

/// The customers the Admin API created: what `created_by_app_id` reads.
pub fn created_by_app(state: &ShopState) -> HashSet<String> {
    state
        .changes()
        .into_iter()
        .filter(|change| change.kind == crate::state::ChangeKind::Inserted)
        .map(|change| change.key.to_string())
        .filter(|key| state.type_of(key) == Some("Customer"))
        .collect()
}

/// The app's numeric id: the app that creates what the Admin API creates.
pub fn app_id(state: &ShopState) -> Option<u64> {
    state
        .keys("App")
        .next()
        .and_then(parse_gid)
        .map(|(_, id)| id)
}

/// What the filters read of a customer.
pub fn facts(state: &ShopState, customer: &Record, created: &HashSet<String>) -> Facts {
    let text = |value: Option<&Json>| value.and_then(Json::as_str).map(str::to_string);
    let id = text(customer.get("id")).unwrap_or_default();
    let addresses: Vec<&Record> = customer
        .get("addressesV2")
        .or_else(|| customer.get("addresses"))
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_object)
        .collect();
    let mut countries = Vec::new();
    let mut regions = Vec::new();
    let mut cities = Vec::new();
    let mut coordinates = Vec::new();
    for address in &addresses {
        let country = text(address.get("countryCodeV2")).unwrap_or_default();
        if country.is_empty() {
            continue;
        }
        countries.push(country.clone());
        let province = text(address.get("provinceCode")).unwrap_or_default();
        if !province.is_empty() {
            regions.push(format!("{country}-{province}"));
        }
        if let Some(city) = text(address.get("city")).filter(|city| !city.is_empty()) {
            cities.push(format!("{country}-{province}-{}", city_code(&city)));
        }
        if let (Some(lat), Some(lng)) = (
            address.get("latitude").and_then(Json::as_f64),
            address.get("longitude").and_then(Json::as_f64),
        ) {
            coordinates.push((lat, lng));
        }
    }
    let orders = customer
        .get("orders")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .filter_map(|order| state.get(order))
        .map(|order| order_facts(state, order))
        .collect();
    let companies = customer
        .get("companyContactProfiles")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .filter_map(|contact| state.get(contact))
        .filter_map(|contact| contact.get("company").and_then(Json::as_str))
        .filter_map(parse_gid)
        .map(|(_, id)| id)
        .collect();
    let credits = state
        .all("StoreCreditAccount")
        .filter(|account| {
            account
                .get("owner")
                .and_then(Json::as_str)
                .is_some_and(|owner| owner == id)
        })
        .map(|account| CreditFacts {
            balance: decimal(account.get("balance").and_then(|b| b.get("amount"))),
            currency: account
                .get("balance")
                .and_then(|b| b.get("currencyCode"))
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            next_expiry: value::date_time_of(account.get("lsfNextExpiryAt")),
            last_credit: value::date_time_of(account.get("lsfLastCreditAt")),
        })
        .collect();
    let metafields = customer
        .get("metafields")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .filter_map(|metafield| state.get(metafield))
        .filter_map(|metafield| {
            Some((
                format!(
                    "{}.{}",
                    metafield.get("namespace")?.as_str()?,
                    metafield.get("key")?.as_str()?
                ),
                metafield.get("value")?.as_str()?.to_string(),
            ))
        })
        .collect();
    let consent_state = |field: &str| {
        customer
            .get(field)
            .and_then(|consent| consent.get("marketingState"))
            .and_then(Json::as_str)
            .map(str::to_string)
    };
    Facts {
        amount_spent: decimal(customer.get("amountSpent").and_then(|a| a.get("amount"))),
        number_of_orders: match customer.get("numberOfOrders") {
            Some(Json::String(n)) => n.parse().unwrap_or(0),
            Some(Json::Number(n)) => n.as_i64().unwrap_or(0),
            _ => 0,
        },
        created_at: value::date_time_of(customer.get("createdAt")),
        account_status: text(customer.get("state")).unwrap_or_else(|| "DISABLED".into()),
        email_domain: text(customer.get("email")).and_then(|email| {
            email
                .rsplit_once('@')
                .map(|(_, domain)| domain.to_lowercase())
        }),
        language: text(customer.get("locale")),
        tags: tags_of(customer.get("tags")),
        email_state: consent_state("emailMarketingConsent"),
        sms_state: consent_state("smsMarketingConsent"),
        countries,
        regions,
        cities,
        coordinates,
        companies,
        created_by_app: created.contains(&id).then(|| app_id(state)).flatten(),
        abandoned_checkout: crate::domains::checkouts::last_abandonment(state, &id),
        product_subscription: text(customer.get("productSubscriberStatus"))
            .unwrap_or_else(|| "NEVER_SUBSCRIBED".into()),
        orders,
        credits,
        metafields,
        id,
    }
}

fn order_facts(state: &ShopState, order: &Record) -> OrderFacts {
    let app_id = order
        .get("app")
        .and_then(|app| {
            app.get("id")
                .and_then(Json::as_str)
                .or_else(|| app.as_str())
        })
        .and_then(parse_gid)
        .map_or(ONLINE_STORE_APP_ID, |(_, id)| id);
    let location_id = ["physicalLocation", "retailLocation"]
        .iter()
        .find_map(|field| order.get(*field).and_then(Json::as_str))
        .and_then(parse_gid)
        .map(|(_, id)| id);
    let lines = order
        .get("lineItems")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .filter_map(|line| state.get(line))
        .map(|line| {
            let product = line.get("product").and_then(Json::as_str);
            let tags = product
                .and_then(|product| state.get(product))
                .map(|product| tags_of(product.get("tags")))
                .unwrap_or_default();
            let quantity = line.get("quantity").and_then(Json::as_i64).unwrap_or(0);
            (
                product.and_then(parse_gid).map(|(_, id)| id),
                tags,
                quantity,
            )
        })
        .collect();
    OrderFacts {
        at: value::date_time_of(order.get("processedAt").or_else(|| order.get("createdAt"))),
        total: decimal(
            order
                .get("totalPriceSet")
                .and_then(|set| set.get("shopMoney"))
                .and_then(|money| money.get("amount")),
        ),
        app_id,
        location_id,
        lines,
    }
}

/// Whether a query keeps a customer.
pub fn matches(query: &Query, facts: &Facts, context: &Context) -> bool {
    expr(&query.root, facts, context)
}

fn expr(expr: &Expr, facts: &Facts, context: &Context) -> bool {
    match expr {
        Expr::And(parts) => parts.iter().all(|part| self::expr(part, facts, context)),
        Expr::Or(parts) => parts.iter().any(|part| self::expr(part, facts, context)),
        Expr::Clause(clause) => self::clause(clause, facts, context),
    }
}

/// A number compared with a condition.
fn compare_number(value: f64, op: Op, condition: &Value) -> bool {
    condition
        .as_f64()
        .and_then(|wanted| value.partial_cmp(&wanted))
        .is_some_and(|ordering| op.holds(ordering))
}

fn between_numbers(value: f64, from: &Value, to: &Value) -> bool {
    match (from.as_f64(), to.as_f64()) {
        (Some(from), Some(to)) => from <= value && value <= to,
        _ => false,
    }
}

fn condition_text(value: &Value) -> String {
    match value {
        Value::Text(text) => text.clone(),
        Value::Number(text) => text.clone(),
        other => format!("{other:?}"),
    }
}

/// A date attribute: absent, it is `NULL` and no comparison holds.
fn date_test(at: Option<DateTime<Utc>>, test: &Test, context: &Context) -> bool {
    match (test, at) {
        (Test::Null(negated), at) => at.is_some() == *negated,
        (Test::Compare(op, Value::Date(date)), Some(at)) => compare_date(context, at, *op, date),
        (Test::Between(Value::Date(from), Value::Date(to)), Some(at)) => {
            between_dates(context, at, from, to)
        }
        _ => false,
    }
}

fn number_test(value: f64, test: &Test) -> bool {
    match test {
        Test::Compare(op, condition) => compare_number(value, *op, condition),
        Test::Between(from, to) => between_numbers(value, from, to),
        _ => false,
    }
}

/// An enum or a text attribute: `=`, `!=`, `IS NULL`. Case counts, as the reference says.
fn text_test(value: Option<&str>, test: &Test) -> bool {
    match (test, value) {
        (Test::Null(negated), value) => value.is_some() == *negated,
        (Test::Compare(Op::Eq, condition), Some(value)) => value == condition_text(condition),
        (Test::Compare(Op::Ne, condition), Some(value)) => value != condition_text(condition),
        (Test::Compare(Op::Ne, _), None) => true,
        _ => false,
    }
}

/// A list attribute: `CONTAINS`, `NOT CONTAINS`, `IS NULL` (an empty list is `NULL`).
fn list_test(values: &[String], test: &Test, ignore_case: bool) -> bool {
    match test {
        Test::Null(negated) => !values.is_empty() == *negated,
        Test::Contains(negated, condition) => {
            let wanted = condition_text(condition);
            let found = values.iter().any(|value| {
                if ignore_case {
                    value.eq_ignore_ascii_case(&wanted)
                } else {
                    *value == wanted
                }
            });
            found != *negated
        }
        _ => false,
    }
}

fn clause(clause: &Clause, facts: &Facts, context: &Context) -> bool {
    let test = &clause.test;
    match clause.attribute.as_str() {
        "abandoned_checkout_date" => date_test(facts.abandoned_checkout, test, context),
        "anniversary" => anniversary(clause, facts, context),
        "amount_spent" => number_test(facts.amount_spent, test),
        "companies" => {
            let ids: Vec<String> = facts.companies.iter().map(u64::to_string).collect();
            list_test(&ids, test, false)
        }
        "created_by_app_id" => match (test, facts.created_by_app) {
            (Test::Compare(Op::Ne, _), None) => true,
            (_, Some(app)) => number_test(app as f64, test),
            _ => false,
        },
        "customer_account_status" => text_test(Some(&facts.account_status), test),
        "customer_added_date" => date_test(facts.created_at, test, context),
        "customer_cities" => list_test(&facts.cities, test, true),
        "customer_countries" => list_test(&facts.countries, test, true),
        "customer_regions" => list_test(&facts.regions, test, true),
        "customer_email_domain" => text_test(facts.email_domain.as_deref(), test),
        "customer_language" => match (test, facts.language.as_deref()) {
            // "The filter value acts as a wildcard if only the language prefix is specified"
            // (Help Center): `en` is also `en-GB`.
            (Test::Compare(op @ (Op::Eq | Op::Ne), condition), Some(language)) => {
                let wanted = condition_text(condition);
                let same = language.eq_ignore_ascii_case(&wanted)
                    || (!wanted.contains('-')
                        && language
                            .split_once('-')
                            .is_some_and(|(prefix, _)| prefix.eq_ignore_ascii_case(&wanted)));
                same == (*op == Op::Eq)
            }
            (test, language) => text_test(language, test),
        },
        "customer_tags" => list_test(&facts.tags, test, true),
        "customer_within_distance" => within_distance(test, facts),
        "email_subscription_status" => text_test(facts.email_state.as_deref(), test),
        "sms_subscription_status" => text_test(facts.sms_state.as_deref(), test),
        "first_order_date" => date_test(facts.first_order(), test, context),
        "last_order_date" => date_test(facts.last_order(), test, context),
        "number_of_orders" => number_test(facts.number_of_orders as f64, test),
        "orders_placed" => orders_placed(test, facts, context),
        "products_purchased" => products_purchased(test, facts, context),
        "product_subscription_status" => text_test(Some(&facts.product_subscription), test),
        // Shopify's models: lsf has no tier nor group for anyone.
        "predicted_spend_tier" | "rfm_group" => text_test(None, test),
        "store_credit_accounts" => store_credit(test, facts, context),
        // lsf sends no Shopify Email and records no storefront view: no customer had one.
        _ => matches!(test, Test::Matches(true, _)),
    }
}

/// `MATCHES` holds when one of the items the function ranges over (or their aggregate) passes
/// every parameter.
fn matched(test: &Test, found: bool) -> bool {
    match test {
        Test::Matches(negated, _) => found != *negated,
        Test::Null(negated) => found == *negated,
        _ => false,
    }
}

fn params(test: &Test) -> &[Param] {
    match test {
        Test::Matches(_, params) => params,
        _ => &[],
    }
}

fn id_param(test: &ParamTest, id: Option<u64>) -> bool {
    let is = |value: &Value| id.is_some_and(|id| value.as_i64() == Some(id as i64));
    match test {
        ParamTest::Compare(Op::Eq, value) => is(value),
        ParamTest::Compare(Op::Ne, value) => !is(value),
        ParamTest::In(negated, values) => values.iter().any(is) != *negated,
        _ => false,
    }
}

fn number_param(test: &ParamTest, value: f64) -> bool {
    match test {
        ParamTest::Compare(op, condition) => compare_number(value, *op, condition),
        ParamTest::Between(from, to) => between_numbers(value, from, to),
        _ => false,
    }
}

fn date_param(test: &ParamTest, at: Option<DateTime<Utc>>, context: &Context) -> bool {
    let Some(at) = at else {
        return false;
    };
    match test {
        ParamTest::Compare(op, Value::Date(date)) => compare_date(context, at, *op, date),
        ParamTest::Between(Value::Date(from), Value::Date(to)) => {
            between_dates(context, at, from, to)
        }
        _ => false,
    }
}

fn text_param(test: &ParamTest, values: &[String]) -> bool {
    let has = |value: &Value| {
        let wanted = condition_text(value);
        values.iter().any(|v| v.eq_ignore_ascii_case(&wanted))
    };
    match test {
        ParamTest::Compare(Op::Eq, value) | ParamTest::Contains(false, value) => has(value),
        ParamTest::Compare(Op::Ne, value) | ParamTest::Contains(true, value) => !has(value),
        _ => false,
    }
}

/// `orders_placed MATCHES (app_id, location_id, amount, date, count, sum_amount)`: the orders
/// that pass `app_id`, `location_id`, `amount` and `date`, then their count and their sum.
fn orders_placed(test: &Test, facts: &Facts, context: &Context) -> bool {
    let params = params(test);
    let kept: Vec<&OrderFacts> = facts
        .orders
        .iter()
        .filter(|order| {
            params.iter().all(|param| match param.name.as_str() {
                "app_id" => id_param(&param.test, Some(order.app_id)),
                "location_id" => id_param(&param.test, order.location_id),
                "amount" => number_param(&param.test, order.total),
                "date" => date_param(&param.test, order.at, context),
                _ => true,
            })
        })
        .collect();
    let aggregates = params.iter().all(|param| match param.name.as_str() {
        "count" => number_param(&param.test, kept.len() as f64),
        "sum_amount" => number_param(&param.test, kept.iter().map(|order| order.total).sum()),
        _ => true,
    });
    let found = !kept.is_empty() && aggregates;
    matched(test, found)
}

/// `products_purchased MATCHES (id, tag, date, quantity, sum_quantity)`: the lines that pass
/// `id`, `tag`, `date` and `quantity` (per order), then the sum of their quantities.
fn products_purchased(test: &Test, facts: &Facts, context: &Context) -> bool {
    let params = params(test);
    let mut quantity = 0i64;
    let mut found = false;
    for order in &facts.orders {
        let date_ok = params
            .iter()
            .filter(|param| param.name == "date")
            .all(|param| date_param(&param.test, order.at, context));
        if !date_ok {
            continue;
        }
        let mut in_order = 0;
        for (product, tags, line_quantity) in &order.lines {
            let line_ok = params.iter().all(|param| match param.name.as_str() {
                "id" => id_param(&param.test, *product),
                "tag" => text_param(&param.test, tags),
                _ => true,
            });
            if line_ok {
                in_order += line_quantity;
            }
        }
        if in_order == 0 {
            continue;
        }
        let per_order = params
            .iter()
            .filter(|param| param.name == "quantity")
            .all(|param| number_param(&param.test, in_order as f64));
        if per_order {
            found = true;
            quantity += in_order;
        }
    }
    let sum_ok = params
        .iter()
        .filter(|param| param.name == "sum_quantity")
        .all(|param| number_param(&param.test, quantity as f64));
    matched(test, found && sum_ok)
}

fn store_credit(test: &Test, facts: &Facts, context: &Context) -> bool {
    let params = params(test);
    let found = facts.credits.iter().any(|account| {
        params.iter().all(|param| match param.name.as_str() {
            "balance" => number_param(&param.test, account.balance),
            "currency" => text_param(&param.test, std::slice::from_ref(&account.currency)),
            "next_expiry_date" => date_param(&param.test, account.next_expiry, context),
            "last_credit_date" => date_param(&param.test, account.last_credit, context),
            _ => true,
        })
    });
    matched(test, found)
}

/// The distance between two points on Earth, in kilometers (haversine).
fn distance_km((lat1, lng1): (f64, f64), (lat2, lng2): (f64, f64)) -> f64 {
    let radians = |degrees: f64| degrees.to_radians();
    let (dlat, dlng) = (radians(lat2 - lat1), radians(lng2 - lng1));
    let a = (dlat / 2.0).sin().powi(2)
        + radians(lat1).cos() * radians(lat2).cos() * (dlng / 2.0).sin().powi(2);
    6371.0 * 2.0 * a.sqrt().asin()
}

fn within_distance(test: &Test, facts: &Facts) -> bool {
    let params = params(test);
    let center = params.iter().find_map(|param| match &param.test {
        ParamTest::Compare(_, Value::Coordinates(lat, lng)) => Some((*lat, *lng)),
        _ => None,
    });
    let radius = params.iter().find_map(|param| {
        let ParamTest::Compare(_, value) = &param.test else {
            return None;
        };
        match param.name.as_str() {
            "distance_km" => value.as_f64(),
            "distance_mi" => value.as_f64().map(|miles| miles * 1.609_344),
            _ => None,
        }
    });
    let found = match (center, radius) {
        (Some(center), Some(radius)) => facts
            .coordinates
            .iter()
            .any(|point| distance_km(center, *point) <= radius),
        _ => false,
    };
    matched(test, found)
}

/// `anniversary('metafields.<namespace>.<key>') <date condition>`: the day and the month of the
/// date metafield, in the year of the condition's date (the next one for an upcoming range).
fn anniversary(clause: &Clause, facts: &Facts, context: &Context) -> bool {
    let key = clause
        .argument
        .as_deref()
        .and_then(|path| path.strip_prefix("metafields."))
        .unwrap_or_default();
    let date = facts
        .metafields
        .iter()
        .find(|(name, _)| name == key)
        .and_then(|(_, value)| NaiveDate::parse_from_str(value.get(..10)?, "%Y-%m-%d").ok());
    let Some(date) = date else {
        return matches!(clause.test, Test::Null(false));
    };
    // The anniversary in the year of today, and in the next one, so that a range across the
    // new year finds it.
    let today = context.today();
    let candidates: Vec<DateTime<Utc>> = [today.year(), today.year() + 1]
        .iter()
        .filter_map(|year| {
            date.with_year(*year)
                .or_else(|| NaiveDate::from_ymd_opt(*year, 2, 28))
        })
        .map(|day| context.start_of(day))
        .collect();
    match &clause.test {
        Test::Null(negated) => *negated,
        test => candidates
            .iter()
            .any(|at| date_test(Some(*at), test, context)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::segments::language::parse;

    fn context() -> Context {
        Context {
            now: Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap(),
            zone: "America/New_York".parse().unwrap(),
        }
    }

    fn keeps(query: &str, facts: &Facts) -> bool {
        matches(&parse(query).unwrap(), facts, &context())
    }

    fn sample() -> Facts {
        Facts {
            id: "gid://shopify/Customer/1".into(),
            amount_spent: 150.0,
            number_of_orders: 2,
            created_at: Some(Utc.with_ymd_and_hms(2026, 1, 15, 3, 0, 0).unwrap()),
            account_status: "ENABLED".into(),
            email_domain: Some("example.com".into()),
            language: Some("en-GB".into()),
            tags: vec!["VIP".into(), "wholesale".into()],
            email_state: Some("SUBSCRIBED".into()),
            sms_state: None,
            countries: vec!["US".into()],
            regions: vec!["US-NY".into()],
            cities: vec!["US-NY-NewYorkCity".into()],
            coordinates: vec![(45.42, -75.69)],
            companies: vec![7],
            created_by_app: None,
            abandoned_checkout: None,
            product_subscription: "NEVER_SUBSCRIBED".into(),
            orders: vec![
                OrderFacts {
                    at: Some(Utc.with_ymd_and_hms(2026, 10, 25, 15, 0, 0).unwrap()),
                    total: 100.0,
                    app_id: ONLINE_STORE_APP_ID,
                    location_id: None,
                    lines: vec![(Some(11), vec!["Sports wear".into()], 2)],
                },
                OrderFacts {
                    at: Some(Utc.with_ymd_and_hms(2025, 5, 30, 15, 0, 0).unwrap()),
                    total: 50.0,
                    app_id: ONLINE_STORE_APP_ID,
                    location_id: None,
                    lines: vec![(Some(12), vec![], 1)],
                },
            ],
            credits: vec![],
            metafields: vec![("facts.birth_date".into(), "1990-11-10".into())],
        }
    }

    #[test]
    fn reads_the_attributes_of_a_customer() {
        let facts = sample();
        for (query, expected) in [
            ("amount_spent > 100 AND customer_tags CONTAINS 'vip'", true),
            ("amount_spent BETWEEN 1 AND 100", false),
            ("number_of_orders = 2", true),
            ("customer_tags NOT CONTAINS 'wholesale'", false),
            ("email_subscription_status = 'SUBSCRIBED'", true),
            ("sms_subscription_status IS NULL", true),
            ("customer_language = 'en'", true),
            ("customer_language != 'fr'", true),
            ("customer_email_domain = 'example.com'", true),
            ("customer_countries CONTAINS 'us'", true),
            ("customer_regions CONTAINS 'US-NY'", true),
            ("customer_cities CONTAINS 'US-NY-NewYorkCity'", true),
            ("companies CONTAINS 7", true),
            ("companies IS NULL", false),
            ("customer_account_status = 'DISABLED'", false),
            ("last_order_date >= -7d", true),
            ("last_order_date = 2026-10-25", true),
            ("first_order_date = 2025-05-30", true),
            ("first_order_date > 2025-05-30", false),
            ("last_order_date BETWEEN 2026-01-01 AND today", true),
            ("abandoned_checkout_date IS NULL", true),
            ("products_purchased MATCHES (id = 11)", true),
            (
                "products_purchased MATCHES (tag = 'sports wear', date >= -30d)",
                true,
            ),
            (
                "products_purchased MATCHES (id IN (12), date >= -30d)",
                false,
            ),
            ("products_purchased MATCHES (sum_quantity >= 3)", true),
            ("products_purchased NOT MATCHES ()", false),
            ("orders_placed MATCHES (count >= 2)", true),
            (
                "orders_placed MATCHES (sum_amount >= 120, date >= -6m)",
                false,
            ),
            ("orders_placed MATCHES (app_id = 580111)", true),
            ("shopify_email.opened MATCHES ()", false),
            ("shopify_email.opened NOT MATCHES ()", true),
            ("rfm_group = 'LOYAL'", false),
            ("predicted_spend_tier IS NULL", true),
            ("store_credit_accounts MATCHES (balance >= 1)", false),
            (
                "customer_within_distance MATCHES (coordinates = (45.419190, -75.696727), distance_mi = 10)",
                true,
            ),
            (
                "anniversary('metafields.facts.birth_date') BETWEEN today AND +30d",
                true,
            ),
            (
                "email_subscription_status = 'PENDING' OR number_of_orders > 1 AND amount_spent < 10",
                false,
            ),
        ] {
            assert_eq!(keeps(query, &facts), expected, "{query}");
        }
    }

    #[test]
    fn dates_are_whole_days_in_the_shop_time_zone() {
        let mut facts = sample();
        // 2026-10-25T03:30Z is still the 24th in New York.
        facts.orders = vec![OrderFacts {
            at: Some(Utc.with_ymd_and_hms(2026, 10, 25, 3, 30, 0).unwrap()),
            ..OrderFacts::default()
        }];
        assert!(keeps("last_order_date = 2026-10-24", &facts));
        assert!(keeps("last_order_date < 2026-10-25", &facts));
        assert!(keeps("last_order_date <= 2026-10-24", &facts));
        assert!(!keeps("last_order_date > 2026-10-24", &facts));
        assert!(keeps("last_order_date > 2026-10-24T23:00:00", &facts));
        assert_eq!(city_code("New York City"), "NewYorkCity");
        assert_eq!(city_code("los angeles"), "LosAngeles");
    }
}
