//! What the family's mutations share: the shapes of a contract and of what it holds (lines,
//! discounts, policies, delivery methods), read from the inputs of the draft API and of the
//! calculation API, the amounts a contract derives (each line's discounted price, the projected
//! order totals), and the errors.
//!
//! A contract, a draft, a calculated contract and the edited contract of a billing cycle share
//! these fields, so the functions here work on any of them (a "contract-like" record).
//!
//! Messages: where shopify.dev documents an error by its code only, the message is the code's
//! description in the 2026-10 schema ([`described`]), as the other families do.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use crate::domains::catalog::gids;
use crate::domains::orders::TAX_RATE_SETTING;
use crate::domains::orders::model::{cents, parse_cents, share};
use crate::resolve::{UserError, coded, input_path};
use crate::state::{Record, ShopState, value};

pub const CONTRACT: &str = "SubscriptionContract";
pub const DRAFT: &str = "SubscriptionDraft";
pub const ATTEMPT: &str = "SubscriptionBillingAttempt";
pub const PAYMENT_METHOD: &str = "CustomerPaymentMethod";
pub const CALCULATION: &str = "SubscriptionContractCalculationSuccess";

/// The statuses a contract does not leave ("Subscription contract status cannot be changed once
/// terminated."): Shopify calls a cancelled or expired contract terminated.
pub const TERMINATED: [&str; 2] = ["CANCELLED", "EXPIRED"];

/// The most lines a contract holds, and the most discounts (`TOO_MANY_LINES`,
/// `TOO_MANY_DISCOUNTS`): Shopify does not publish its limits; these are lsf's.
pub const MAX_LINES: usize = 250;
pub const MAX_DISCOUNTS: usize = 250;

/// The longest note a contract keeps (`INVALID_NOTE_LENGTH`): lsf's limit, Shopify's is not
/// published.
pub const MAX_NOTE: usize = 5000;

/// The description the schema gives a value of an enum: the message of an error Shopify
/// documents by its code only.
pub fn described(enum_name: &str, code: &str) -> String {
    crate::schema::admin()
        .get(enum_name)
        .and_then(|def| def.enum_values.get(code))
        .and_then(|value| value.description.clone())
        .unwrap_or_else(|| code.to_string())
}

/// A user error of the draft API (`SubscriptionDraftUserError`), worded as its code's
/// description.
pub fn draft_error(field: &[&str], code: &str) -> UserError {
    coded(field, described("SubscriptionDraftErrorCode", code), code)
}

/// A user error coded `INVALID`, with its message: lsf's wording, for what Shopify documents no
/// message of (a record that does not exist, a value out of range).
pub fn invalid_error(field: &[&str], message: impl Into<String>) -> UserError {
    coded(field, message, "INVALID")
}

/// A `SubscriptionContractStatusUpdateUserError`.
pub fn status_error(field: &[&str], code: &str) -> UserError {
    coded(
        field,
        described("SubscriptionContractStatusUpdateErrorCode", code),
        code,
    )
}

/// A `BillingAttemptUserError`.
pub fn attempt_error(field: &[&str], code: &str) -> UserError {
    coded(field, described("BillingAttemptUserErrorCode", code), code)
}

/// A `SubscriptionBillingCycleUserError`.
pub fn cycle_error(field: &[&str], code: &str) -> UserError {
    coded(
        field,
        described("SubscriptionBillingCycleErrorCode", code),
        code,
    )
}

/// `pointer` for a record, as `Value::pointer` reads a value: `/state/__typename`.
pub trait Pointer {
    fn pointer(&self, path: &str) -> Option<&Value>;
}

impl Pointer for Map<String, Value> {
    fn pointer(&self, path: &str) -> Option<&Value> {
        let path = path.strip_prefix('/')?;
        match path.split_once('/') {
            Some((first, rest)) => self.get(first)?.pointer(&format!("/{rest}")),
            None => self.get(path),
        }
    }
}

/// A string field of a record or an input.
pub fn text<'r>(record: &'r Map<String, Value>, field: &str) -> Option<&'r str> {
    record.get(field).and_then(Value::as_str)
}

/// An input object (`{}` when absent).
pub fn object(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The objects of a list.
pub fn objects(value: Option<&Value>) -> Vec<Map<String, Value>> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .cloned()
        .collect()
}

/// A `DateTime` input, in UTC.
pub fn date_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

/// The number of a GID (`5` of `gid://shopify/SubscriptionContract/5`), for the ids made of it.
pub fn number(gid: &str) -> u64 {
    crate::state::parse_gid(gid).map_or(0, |(_, id)| id)
}

/// A new id of an object that is not a record, in Shopify's form for it
/// (`gid://shopify/SubscriptionLine/<uuid>`): a UUID made of the shop and a counter of the
/// type, so the same requests give the same ids.
pub fn uuid_gid(state: &mut ShopState, type_name: &str) -> String {
    let counter = state.new_gid(type_name);
    let key = format!("{}/{counter}", state.domain());
    format!(
        "gid://shopify/{type_name}/{}",
        value::uuid(&format!("{type_name}/{key}"))
    )
}

/// A `MoneyV2` of the contract's currency.
pub fn money(amount: i64, currency: &str) -> Value {
    value::money_v2(amount, currency)
}

/// The cents of a `Decimal` input (`"9.99"`, `9.99`), `None` when it is not one.
pub fn decimal_cents(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::String(text) => parse_cents(text),
        Value::Number(number) => number
            .as_f64()
            .map(|amount| (amount * 100.0).round() as i64),
        _ => None,
    }
}

/// Whether a currency is one the shop sells in: its own, or one of its presentment currencies.
pub fn currency_enabled(state: &ShopState, currency: &str) -> bool {
    let shop = state.shop();
    shop.and_then(|shop| text(shop, "currencyCode")) == Some(currency)
        || shop
            .and_then(|shop| shop.get("enabledPresentmentCurrencies"))
            .and_then(Value::as_array)
            .is_some_and(|currencies| currencies.iter().any(|known| known == currency))
}

// ---- policies -------------------------------------------------------------------------------

/// The `SellingPlanAnchor`s of an anchor input list, from the draft API's `SellingPlanAnchorInput`
/// (`{type, day, month, cutoffDay}`) or the calculation API's `@oneOf` anchors (`monthday`,
/// `weekday`, `yearday`).
pub fn anchors(value: Option<&Value>) -> Value {
    let weekday = |day: &str| -> i64 {
        match day {
            "MONDAY" => 1,
            "TUESDAY" => 2,
            "WEDNESDAY" => 3,
            "THURSDAY" => 4,
            "FRIDAY" => 5,
            "SATURDAY" => 6,
            _ => 7,
        }
    };
    let anchors: Vec<Value> = objects(value)
        .iter()
        .map(|anchor| {
            if let Some(monthday) = anchor.get("monthday") {
                json!({"type": "MONTHDAY", "day": monthday.get("dayOfMonth"), "month": null, "cutoffDay": null})
            } else if let Some(weekday_input) = anchor.get("weekday") {
                let day = weekday_input.get("dayOfWeek").and_then(Value::as_str).unwrap_or("MONDAY");
                json!({"type": "WEEKDAY", "day": weekday(day), "month": null, "cutoffDay": null})
            } else if let Some(yearday) = anchor.get("yearday") {
                json!({"type": "YEARDAY", "day": yearday.get("dayOfMonth"), "month": yearday.get("month"), "cutoffDay": null})
            } else {
                json!({
                    "type": anchor.get("type").cloned().unwrap_or(json!("MONTHDAY")),
                    "day": anchor.get("day").cloned().unwrap_or(json!(1)),
                    "month": anchor.get("month"),
                    "cutoffDay": anchor.get("cutoffDay"),
                })
            }
        })
        .collect();
    Value::Array(anchors)
}

/// The errors of anchors (`path` the anchors' input): a day out of the range of its type.
pub fn check_anchors(anchors: &Value, path: &[&str], errors: &mut Vec<UserError>) {
    for (index, anchor) in anchors.as_array().into_iter().flatten().enumerate() {
        let day = anchor.get("day").and_then(Value::as_i64).unwrap_or(0);
        let month = anchor.get("month").and_then(Value::as_i64);
        let valid = match anchor.get("type").and_then(Value::as_str) {
            Some("WEEKDAY") => (1..=7).contains(&day),
            Some("MONTHDAY") => (1..=31).contains(&day),
            Some("YEARDAY") => {
                (1..=31).contains(&day) && month.is_some_and(|m| (1..=12).contains(&m))
            }
            _ => false,
        };
        if !valid {
            let index = index.to_string();
            errors.push(draft_error(&input_path(path, &[&index]), "NOT_IN_RANGE"));
        }
    }
}

/// A `SubscriptionBillingPolicy` from the draft API's input.
pub fn billing_policy(input: &Map<String, Value>) -> Value {
    let interval = text(input, "interval").unwrap_or("MONTH");
    let count = input
        .get("intervalCount")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    json!({
        "interval": interval,
        "intervalCount": count,
        "anchors": anchors(input.get("anchors")),
        "minCycles": input.get("minCycles"),
        "maxCycles": input.get("maxCycles"),
        "cadence": {"count": count, "unit": interval},
    })
}

/// A `SubscriptionDeliveryPolicy` from the draft API's input.
pub fn delivery_policy(input: &Map<String, Value>) -> Value {
    json!({
        "interval": text(input, "interval").unwrap_or("MONTH"),
        "intervalCount": input.get("intervalCount").and_then(Value::as_i64).unwrap_or(1),
        "anchors": anchors(input.get("anchors")),
        "multiFulfillment": null,
    })
}

/// The policies of a calculation input: its `billingPolicy` (`cadence`, `anchors`), its
/// `deliveryPolicy` (`anchors`, `multiFulfillment`), and the `appManagedBillingConfig`'s cycles.
/// The delivery cadence is the billing one unless `multiFulfillment` gives another.
pub fn calculated_policies(
    billing: &Map<String, Value>,
    delivery: &Map<String, Value>,
    config: &Map<String, Value>,
) -> (Value, Value) {
    let cadence = object(billing.get("cadence"));
    let unit = text(&cadence, "unit").unwrap_or("MONTH");
    let count = cadence.get("count").and_then(Value::as_i64).unwrap_or(1);
    let billing_policy = json!({
        "interval": unit,
        "intervalCount": count,
        "anchors": anchors(billing.get("anchors")),
        "minCycles": config.get("minCycles"),
        "maxCycles": config.get("maxCycles"),
        "cadence": {"count": count, "unit": unit},
    });
    let multi = delivery
        .get("multiFulfillment")
        .filter(|value| !value.is_null());
    let (delivery_unit, delivery_count) = match multi {
        Some(multi) => {
            let cadence = object(multi.get("cadence"));
            (
                text(&cadence, "unit").unwrap_or(unit).to_string(),
                cadence
                    .get("count")
                    .and_then(Value::as_i64)
                    .unwrap_or(count),
            )
        }
        None => (unit.to_string(), count),
    };
    let delivery_policy = json!({
        "interval": delivery_unit,
        "intervalCount": delivery_count,
        "anchors": anchors(delivery.get("anchors")),
        "multiFulfillment": multi.map(|multi| json!({
            "cadence": {
                "count": multi.pointer("/cadence/count").cloned().unwrap_or(json!(1)),
                "unit": multi.pointer("/cadence/unit").cloned().unwrap_or(json!("MONTH")),
            },
            "numberOfFulfillments": multi.get("numberOfFulfillments").cloned().unwrap_or(json!(2)),
        })),
    });
    (billing_policy, delivery_policy)
}

/// An interval in days (for `DAY` and `WEEK`) or in months (`MONTH`, `YEAR`), and which.
fn span(policy: &Value) -> (bool, i64) {
    let count = policy
        .get("intervalCount")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    match policy.get("interval").and_then(Value::as_str) {
        Some("DAY") => (false, count),
        Some("WEEK") => (false, 7 * count),
        Some("YEAR") => (true, 12 * count),
        _ => (true, count),
    }
}

/// The errors of a billing and a delivery policy (`path` the input of the contract):
/// counts of at least 1, `maxCycles` above `minCycles`, the billing interval a whole number of
/// delivery intervals (a prepaid contract delivers several times a cycle).
pub fn check_policies(
    billing: Option<&Value>,
    delivery: Option<&Value>,
    path: &[&str],
    errors: &mut Vec<UserError>,
) {
    if let Some(billing) = billing {
        let push = |errors: &mut Vec<UserError>, field: &str, code: &str| {
            errors.push(draft_error(
                &input_path(path, &["billingPolicy", field]),
                code,
            ));
        };
        if billing
            .get("intervalCount")
            .and_then(Value::as_i64)
            .unwrap_or(1)
            < 1
        {
            push(errors, "intervalCount", "GREATER_THAN");
        }
        let min = billing.get("minCycles").and_then(Value::as_i64);
        let max = billing.get("maxCycles").and_then(Value::as_i64);
        if min.is_some_and(|min| min < 1) {
            push(errors, "minCycles", "GREATER_THAN");
        }
        if max.is_some_and(|max| max < 1) {
            push(errors, "maxCycles", "GREATER_THAN");
        }
        if let (Some(min), Some(max)) = (min, max)
            && min > max
        {
            push(
                errors,
                "maxCycles",
                "SELLING_PLAN_MAX_CYCLES_MUST_BE_GREATER_THAN_MIN_CYCLES",
            );
        }
        check_anchors(
            &billing["anchors"],
            &input_path(path, &["billingPolicy", "anchors"]),
            errors,
        );
    }
    if let Some(delivery) = delivery {
        if delivery
            .get("intervalCount")
            .and_then(Value::as_i64)
            .unwrap_or(1)
            < 1
        {
            errors.push(draft_error(
                &input_path(path, &["deliveryPolicy", "intervalCount"]),
                "GREATER_THAN",
            ));
        }
        check_anchors(
            &delivery["anchors"],
            &input_path(path, &["deliveryPolicy", "anchors"]),
            errors,
        );
    }
    if let (Some(billing), Some(delivery)) = (billing, delivery) {
        let (billing_months, billing_span) = span(billing);
        let (delivery_months, delivery_span) = span(delivery);
        // Days and months do not divide one another: only the same kind of interval is checked.
        if billing_months == delivery_months
            && delivery_span > 0
            && billing_span > 0
            && (billing_span < delivery_span || billing_span % delivery_span != 0)
        {
            errors.push(draft_error(
                &input_path(path, &["deliveryPolicy"]),
                "DELIVERY_MUST_BE_MULTIPLE_OF_BILLING",
            ));
        }
    }
}

// ---- delivery -------------------------------------------------------------------------------

/// A `MailingAddress` of a contract from a `MailingAddressInput`, with an id of the shop's
/// address counter.
pub fn address(state: &mut ShopState, input: &Value) -> Option<Value> {
    let number = number(&state.new_gid("MailingAddress"));
    crate::domains::orders::model::mailing_address(
        input,
        &format!("gid://shopify/MailingAddress/{number}?model_name=Address"),
    )
    .map(located)
}

/// An address with what lsf does not know of it said: no coordinates, no time zone, not
/// validated (Shopify geocodes and validates addresses; lsf does not).
pub fn located(mut address: Value) -> Value {
    if let Some(map) = address.as_object_mut() {
        let code = map.get("countryCodeV2").cloned().unwrap_or(Value::Null);
        map.entry("countryCode").or_insert(code);
        map.entry("coordinatesValidated").or_insert(json!(false));
        for field in [
            "latitude",
            "longitude",
            "timeZone",
            "validationResultSummary",
        ] {
            map.entry(field).or_insert(Value::Null);
        }
    }
    address
}

/// The code and titles of a delivery method's option, as its input gives them.
fn delivery_option(input: Option<&Value>) -> Value {
    let field = |name: &str| input.and_then(|input| input.get(name));
    json!({
        "code": field("code"),
        "description": field("description"),
        "presentmentTitle": field("presentmentTitle"),
        "title": field("title"),
    })
}

/// A `SubscriptionDeliveryMethod` from the draft API's input (`shipping`, `localDelivery`,
/// `pickup`), or its errors (`path` the input's).
pub fn delivery_method(
    state: &mut ShopState,
    input: &Map<String, Value>,
    path: &[&str],
) -> Result<Value, Vec<UserError>> {
    if let Some(shipping) = input.get("shipping").filter(|value| !value.is_null()) {
        let address = shipping
            .get("address")
            .and_then(|address| self::address(state, address))
            .unwrap_or(Value::Null);
        let mut shipping_option = delivery_option(shipping.get("shippingOption"));
        // Deprecated: "This field has never been implemented."
        shipping_option["carrierService"] = Value::Null;
        return Ok(json!({
            "__typename": "SubscriptionDeliveryMethodShipping",
            "address": address,
            "shippingOption": shipping_option,
        }));
    }
    if let Some(local) = input.get("localDelivery").filter(|value| !value.is_null()) {
        let Some(delivery_option) = local
            .get("localDeliveryOption")
            .filter(|value| !value.is_null())
        else {
            return Err(vec![draft_error(
                &input_path(path, &["localDelivery", "localDeliveryOption"]),
                "MISSING_LOCAL_DELIVERY_OPTIONS",
            )]);
        };
        let address = local
            .get("address")
            .and_then(|address| self::address(state, address))
            .unwrap_or(Value::Null);
        let mut option = self::delivery_option(Some(delivery_option));
        option["phone"] = delivery_option.get("phone").cloned().unwrap_or(json!(""));
        option["instructions"] = delivery_option
            .get("instructions")
            .cloned()
            .unwrap_or(Value::Null);
        return Ok(json!({
            "__typename": "SubscriptionDeliveryMethodLocalDelivery",
            "address": address,
            "localDeliveryOption": option,
        }));
    }
    if let Some(pickup) = input.get("pickup").filter(|value| !value.is_null()) {
        let pickup_option = pickup.get("pickupOption");
        let location = pickup_option
            .and_then(|option| option.get("locationId"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if state.type_of(location) != Some("Location") {
            return Err(vec![draft_error(
                &input_path(path, &["pickup", "pickupOption", "locationId"]),
                "INVALID",
            )]);
        }
        let mut option = delivery_option(pickup_option);
        option["location"] = json!(location);
        return Ok(json!({
            "__typename": "SubscriptionDeliveryMethodPickup",
            "pickupOption": option,
        }));
    }
    Ok(Value::Null)
}

/// A delivery method and its price from the calculation API's input (`shipping`,
/// `localDelivery`, `pickup` each with its `deliveryPrice`; `none`). `Ok(None)` for
/// `fetchAvailableDeliveryOptions`, which chooses no method.
#[allow(clippy::type_complexity)]
pub fn calculated_delivery_method(
    state: &mut ShopState,
    input: &Map<String, Value>,
    path: &[&str],
) -> Result<Option<(Value, i64)>, Vec<UserError>> {
    if input
        .get("fetchAvailableDeliveryOptions")
        .is_some_and(|value| !value.is_null())
    {
        return Ok(None);
    }
    if input.get("none").and_then(Value::as_bool) == Some(true) {
        return Ok(Some((Value::Null, 0)));
    }
    let price = |method: &Value| cents(method.get("deliveryPrice"));
    if let Some(shipping) = input.get("shipping").filter(|value| !value.is_null()) {
        let address = shipping
            .get("address")
            .and_then(|address| self::address(state, address));
        return Ok(Some((
            json!({
                "__typename": "SubscriptionDeliveryMethodShipping",
                "address": address,
                "shippingOption": delivery_option(Some(shipping)),
            }),
            price(shipping),
        )));
    }
    if let Some(local) = input.get("localDelivery").filter(|value| !value.is_null()) {
        let address = local
            .get("address")
            .and_then(|address| self::address(state, address));
        let mut local_option = delivery_option(Some(local));
        local_option["phone"] = local.get("phone").cloned().unwrap_or(json!(""));
        local_option["instructions"] = local.get("instructions").cloned().unwrap_or(Value::Null);
        return Ok(Some((
            json!({
                "__typename": "SubscriptionDeliveryMethodLocalDelivery",
                "address": address,
                "localDeliveryOption": local_option,
            }),
            price(local),
        )));
    }
    if let Some(pickup) = input.get("pickup").filter(|value| !value.is_null()) {
        let location = pickup
            .get("locationId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if state.type_of(location) != Some("Location") {
            return Err(vec![invalid_error(
                &input_path(path, &["pickup", "locationId"]),
                "Location does not exist",
            )]);
        }
        let mut pickup_option = delivery_option(Some(pickup));
        pickup_option["location"] = json!(location);
        return Ok(Some((
            json!({
                "__typename": "SubscriptionDeliveryMethodPickup",
                "pickupOption": pickup_option,
            }),
            price(pickup),
        )));
    }
    Ok(Some((Value::Null, 0)))
}

/// The address a contract delivers to, when its method has one.
pub fn delivery_address(contract: &Record) -> Option<&Value> {
    contract
        .get("deliveryMethod")
        .and_then(|method| method.get("address"))
        .filter(|address| !address.is_null())
}

// ---- lines ----------------------------------------------------------------------------------

/// What a line takes from its variant: the product, the titles, the SKU, whether it ships and is
/// taxed, its image and weight. `None` when the variant is not one of the shop.
pub fn variant_fields(state: &ShopState, variant: &str) -> Option<Map<String, Value>> {
    if state.type_of(variant) != Some("ProductVariant") {
        return None;
    }
    let record = state.get(variant)?;
    let product = text(record, "product").unwrap_or_default();
    let product_record = state.get(product);
    let item = text(record, "inventoryItem").and_then(|item| state.get(item));
    let variant_title = text(record, "title").filter(|title| *title != "Default Title");
    let mut fields = Map::new();
    fields.insert("productId".into(), json!(product));
    fields.insert("variantId".into(), json!(variant));
    fields.insert(
        "title".into(),
        json!(
            product_record
                .and_then(|product| text(product, "title"))
                .unwrap_or_default()
        ),
    );
    fields.insert("variantTitle".into(), json!(variant_title));
    fields.insert(
        "sku".into(),
        record.get("sku").cloned().unwrap_or(Value::Null),
    );
    fields.insert(
        "requiresShipping".into(),
        json!(
            item.and_then(|item| item.get("requiresShipping"))
                .and_then(Value::as_bool)
                .unwrap_or(true)
        ),
    );
    fields.insert(
        "taxable".into(),
        json!(
            record
                .get("taxable")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        ),
    );
    fields.insert(
        "variantImage".into(),
        record
            .get("image")
            .filter(|image| !image.is_null())
            .cloned()
            .or_else(|| product_record.and_then(|product| product.get("featuredImage").cloned()))
            .unwrap_or(Value::Null),
    );
    fields.insert(
        "weight".into(),
        item.and_then(|item| item.pointer("/measurement/weight"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    fields.insert("lsfPrice".into(), json!(cents(record.get("price"))));
    Some(fields)
}

/// A `SubscriptionPricingPolicy` from the draft API's `SubscriptionPricingPolicyInput`
/// (`Decimal`s) or the calculation API's `appManagedPricingPolicy` (`MoneyInput`s,
/// `cyclePriceAdjustments`).
pub fn pricing_policy(input: &Value, currency: &str) -> Value {
    if input.is_null() {
        return Value::Null;
    }
    let adjustments = input
        .get("cycleDiscounts")
        .or_else(|| input.get("cyclePriceAdjustments"));
    let cycle_discounts: Vec<Value> = objects(adjustments)
        .iter()
        .map(|adjustment| {
            let kind = text(adjustment, "adjustmentType").unwrap_or("PERCENTAGE");
            let given = object(adjustment.get("adjustmentValue"));
            let adjustment_value = if kind == "PERCENTAGE" {
                json!({
                    "__typename": "SellingPlanPricingPolicyPercentageValue",
                    "percentage": given.get("percentage").and_then(Value::as_f64).unwrap_or(0.0),
                })
            } else {
                let mut money = money(
                    decimal_cents(given.get("fixedValue")).unwrap_or(0),
                    currency,
                );
                money["__typename"] = json!("MoneyV2");
                money
            };
            json!({
                "adjustmentType": kind,
                "adjustmentValue": adjustment_value,
                "afterCycle": adjustment.get("afterCycle").cloned().unwrap_or(json!(0)),
                "computedPrice": money(cents(adjustment.get("computedPrice")), currency),
            })
        })
        .collect();
    json!({
        "basePrice": money(cents(input.get("basePrice")), currency),
        "cycleDiscounts": cycle_discounts,
    })
}

/// The errors of a pricing policy (`path` its input): `afterCycle` unique among its adjustments,
/// an adjustment value of its type.
pub fn check_pricing_policy(input: &Value, path: &[&str], errors: &mut Vec<UserError>) {
    let adjustments = objects(
        input
            .get("cycleDiscounts")
            .or_else(|| input.get("cyclePriceAdjustments")),
    );
    let mut seen = Vec::new();
    for (index, adjustment) in adjustments.iter().enumerate() {
        let index = index.to_string();
        let after = adjustment
            .get("afterCycle")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if seen.contains(&after) {
            errors.push(draft_error(path, "CYCLE_DISCOUNTS_UNIQUE_AFTER_CYCLE"));
        }
        seen.push(after);
        let given = object(adjustment.get("adjustmentValue"));
        let percentage = given
            .get("percentage")
            .is_some_and(|value| !value.is_null());
        let fixed = given
            .get("fixedValue")
            .is_some_and(|value| !value.is_null());
        let kind = text(adjustment, "adjustmentType").unwrap_or("PERCENTAGE");
        let at = input_path(path, &[&index, "adjustmentValue"]);
        if percentage == fixed {
            errors.push(draft_error(&at, "INVALID_ADJUSTMENT_VALUE"));
        } else if (kind == "PERCENTAGE") != percentage {
            errors.push(draft_error(&at, "INVALID_ADJUSTMENT_TYPE"));
        }
    }
}

/// The selling plan's name, when the line names a plan of the shop.
pub fn selling_plan_name(state: &ShopState, plan: Option<&str>) -> Option<String> {
    let plan = plan?;
    state
        .get(plan)
        .filter(|_| state.type_of(plan) == Some("SellingPlan"))
        .and_then(|record| text(record, "name"))
        .map(str::to_string)
}

/// A new `SubscriptionLine` from the draft API's `SubscriptionLineInput` (`path` the input's),
/// or its errors.
pub fn new_line(
    state: &mut ShopState,
    input: &Map<String, Value>,
    currency: &str,
    path: &[&str],
) -> Result<Value, Vec<UserError>> {
    let mut errors = Vec::new();
    let variant = text(input, "productVariantId").unwrap_or_default();
    let fields = variant_fields(state, variant);
    if fields.is_none() {
        errors.push(invalid_error(
            &input_path(path, &["productVariantId"]),
            "Product variant does not exist",
        ));
    }
    let quantity = input.get("quantity").and_then(Value::as_i64).unwrap_or(0);
    if quantity < 1 {
        errors.push(draft_error(
            &input_path(path, &["quantity"]),
            "GREATER_THAN_OR_EQUAL_TO",
        ));
    }
    let price = decimal_cents(input.get("currentPrice"));
    match price {
        Some(price) if price < 0 => errors.push(draft_error(
            &input_path(path, &["currentPrice"]),
            "GREATER_THAN_OR_EQUAL_TO",
        )),
        Some(price) if !value::money_in_bounds(price.saturating_mul(quantity.max(1))) => errors
            .push(invalid_error(
                &input_path(path, &["currentPrice"]),
                value::money_too_large("Price"),
            )),
        None => errors.push(draft_error(&input_path(path, &["currentPrice"]), "BLANK")),
        _ => {}
    }
    let plan = text(input, "sellingPlanId");
    if let Some(plan) = plan
        && state.type_of(plan) != Some("SellingPlan")
    {
        errors.push(invalid_error(
            &input_path(path, &["sellingPlanId"]),
            "Selling plan does not exist",
        ));
    }
    if let Some(policy) = input
        .get("pricingPolicy")
        .filter(|policy| !policy.is_null())
    {
        check_pricing_policy(
            policy,
            &input_path(path, &["pricingPolicy", "cycleDiscounts"]),
            &mut errors,
        );
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let fields = fields.unwrap_or_default();
    let id = uuid_gid(state, "SubscriptionLine");
    Ok(line_record(
        state,
        id,
        fields,
        quantity,
        price.unwrap_or(0),
        input,
        currency,
    ))
}

/// A line's record: the variant's fields, the quantity and price, and what the input gives of
/// its plan, its pricing policy and its attributes.
pub fn line_record(
    state: &ShopState,
    id: String,
    fields: Map<String, Value>,
    quantity: i64,
    price: i64,
    input: &Map<String, Value>,
    currency: &str,
) -> Value {
    let plan = text(input, "sellingPlanId").or_else(|| text(input, "originSellingPlanId"));
    let plan_name = text(input, "sellingPlanName")
        .map(str::to_string)
        .or_else(|| selling_plan_name(state, plan));
    let mut line = fields;
    line.remove("lsfPrice");
    line.insert("id".into(), json!(id));
    line.insert("quantity".into(), json!(quantity));
    line.insert("currentPrice".into(), money(price, currency));
    line.insert(
        "lineDiscountedPrice".into(),
        money(price * quantity, currency),
    );
    line.insert("discountAllocations".into(), json!([]));
    line.insert(
        "pricingPolicy".into(),
        pricing_policy(
            input
                .get("pricingPolicy")
                .or_else(|| input.get("appManagedPricingPolicy"))
                .unwrap_or(&Value::Null),
            currency,
        ),
    );
    line.insert("sellingPlanId".into(), json!(plan));
    line.insert("sellingPlanName".into(), json!(plan_name));
    line.insert(
        "customAttributes".into(),
        crate::domains::orders::model::attributes(input.get("customAttributes")),
    );
    line.insert("concatenatedOriginContract".into(), Value::Null);
    Value::Object(line)
}

/// Applies the draft API's `SubscriptionLineUpdateInput` to a line (a patch: what it omits
/// stays), or its errors.
pub fn update_line(
    state: &ShopState,
    line: &mut Value,
    input: &Map<String, Value>,
    currency: &str,
    path: &[&str],
) -> Result<(), Vec<UserError>> {
    let mut errors = Vec::new();
    if let Some(variant) = text(input, "productVariantId") {
        match variant_fields(state, variant) {
            Some(mut fields) => {
                fields.remove("lsfPrice");
                for (key, value) in fields {
                    line[key.as_str()] = value;
                }
            }
            None => errors.push(invalid_error(
                &input_path(path, &["productVariantId"]),
                "Product variant does not exist",
            )),
        }
    }
    if let Some(quantity) = input.get("quantity").filter(|value| !value.is_null()) {
        match quantity.as_i64() {
            Some(quantity) if quantity >= 1 => line["quantity"] = json!(quantity),
            _ => errors.push(draft_error(
                &input_path(path, &["quantity"]),
                "GREATER_THAN_OR_EQUAL_TO",
            )),
        }
    }
    if input
        .get("currentPrice")
        .is_some_and(|value| !value.is_null())
    {
        match decimal_cents(input.get("currentPrice")) {
            Some(price) if price >= 0 && value::money_in_bounds(price) => {
                line["currentPrice"] = money(price, currency);
            }
            _ => errors.push(draft_error(
                &input_path(path, &["currentPrice"]),
                "GREATER_THAN_OR_EQUAL_TO",
            )),
        }
    }
    if let Some(plan) = input.get("sellingPlanId") {
        if let Some(plan) = plan.as_str()
            && state.type_of(plan) != Some("SellingPlan")
        {
            errors.push(invalid_error(
                &input_path(path, &["sellingPlanId"]),
                "Selling plan does not exist",
            ));
        }
        line["sellingPlanId"] = plan.clone();
        if !input.contains_key("sellingPlanName") {
            line["sellingPlanName"] = json!(selling_plan_name(state, plan.as_str()));
        }
    }
    if let Some(name) = input.get("sellingPlanName") {
        line["sellingPlanName"] = name.clone();
    }
    if let Some(attributes) = input.get("customAttributes") {
        line["customAttributes"] = crate::domains::orders::model::attributes(Some(attributes));
    }
    if let Some(policy) = input.get("pricingPolicy") {
        if !policy.is_null() {
            check_pricing_policy(
                policy,
                &input_path(path, &["pricingPolicy", "cycleDiscounts"]),
                &mut errors,
            );
        }
        line["pricingPolicy"] = pricing_policy(policy, currency);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// The items of an array field of a record, none when it holds no array.
fn array<'r>(record: &'r Record, field: &str) -> &'r [Value] {
    record
        .get(field)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The lines of a contract-like record.
pub fn lines(record: &Record) -> Vec<Value> {
    array(record, "lines").to_vec()
}

/// The line of a contract-like record by its id.
pub fn line_of<'r>(record: &'r Record, id: &str) -> Option<&'r Value> {
    array(record, "lines")
        .iter()
        .find(|line| line.get("id").and_then(Value::as_str) == Some(id))
}

// ---- discounts ------------------------------------------------------------------------------

/// The value of a `SubscriptionManualDiscountValueInput` (`percentage` or `fixedAmount`), or
/// `None` when it gives neither.
pub fn discount_value(input: &Map<String, Value>, currency: &str) -> Option<Value> {
    let value = object(input.get("value"));
    if let Some(percentage) = value.get("percentage").and_then(Value::as_i64) {
        return Some(
            json!({"__typename": "SubscriptionDiscountPercentageValue", "percentage": percentage}),
        );
    }
    let fixed = object(value.get("fixedAmount"));
    if !fixed.is_empty() {
        return Some(json!({
            "__typename": "SubscriptionDiscountFixedAmountValue",
            "amount": money(cents(fixed.get("amount")), currency),
            "appliesOnEachItem": fixed.get("appliesOnEachItem").and_then(Value::as_bool).unwrap_or(false),
        }));
    }
    None
}

/// A `SubscriptionManualDiscount` on lines (`targetType: LINE_ITEM`) or on the delivery
/// (`SHIPPING_LINE`).
pub fn manual_discount(
    id: String,
    title: Option<&str>,
    target: &str,
    value: Value,
    limit: Option<i64>,
    kind: &str,
    entitled: (bool, Vec<String>),
) -> Value {
    json!({
        "__typename": "SubscriptionManualDiscount",
        "id": id,
        "title": title,
        "type": kind,
        "targetType": target,
        "value": value,
        "recurringCycleLimit": limit,
        "usageCount": 0,
        "rejectionReason": null,
        "entitledLines": {"all": entitled.0, "lines": [], "lsfLineIds": entitled.1},
    })
}

/// The discounts of a contract-like record.
pub fn discounts(record: &Record) -> Vec<Value> {
    array(record, "discounts").to_vec()
}

/// Whether a discount still applies: not rejected, and not used up (`recurringCycleLimit`
/// above 0 and reached by `usageCount`).
pub fn applies(discount: &Value) -> bool {
    let limit = discount
        .get("recurringCycleLimit")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let used = discount
        .get("usageCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    discount.get("rejectionReason").is_none_or(Value::is_null)
        && discount.get("__typename").and_then(Value::as_str)
            != Some("SubscriptionAppliedCodeDiscount")
        && (limit <= 0 || used < limit)
}

/// The discount without what it lists of the lines: what an allocation names.
fn allocated(discount: &Value) -> Value {
    let mut discount = discount.clone();
    if let Some(entitled) = discount
        .get_mut("entitledLines")
        .and_then(Value::as_object_mut)
    {
        entitled.insert("lines".into(), json!([]));
    }
    discount
}

/// Whether a discount is on a line: on all of them, or on those it lists.
fn entitles(discount: &Value) -> impl Fn(&Value) -> bool + use<> {
    let all = discount
        .pointer("/entitledLines/all")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let listed = gids(discount.pointer("/entitledLines/lsfLineIds"));
    move |line| {
        all || line
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| listed.iter().any(|known| known == id))
    }
}

/// What a line costs before discounts, in cents.
pub fn line_total(line: &Value) -> i64 {
    let quantity = line.get("quantity").and_then(Value::as_i64).unwrap_or(0);
    cents(line.get("currentPrice")).saturating_mul(quantity)
}

/// Recomputes what a contract-like record derives from its lines and discounts: each line's
/// `discountAllocations` and `lineDiscountedPrice` (a percentage of each entitled line, a fixed
/// amount on each item or spread over the entitled lines by their totals, the remainder on the
/// last), each discount's `entitledLines.lines`, and `latestCommittedProjectedOrderTotals` on a
/// contract.
pub fn refresh(state: &ShopState, record: &mut Record) {
    let currency = text(record, "currencyCode").unwrap_or("USD").to_string();
    let mut lines = lines(record);
    let discounts = discounts(record);
    let mut taken: Vec<Vec<Value>> = vec![Vec::new(); lines.len()];
    for discount in discounts.iter().filter(|discount| applies(discount)) {
        if discount.get("targetType").and_then(Value::as_str) == Some("SHIPPING_LINE") {
            continue;
        }
        let entitles = entitles(discount);
        let entitled: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| entitles(line))
            .map(|(index, _)| index)
            .collect();
        let value = &discount["value"];
        let mut amounts = vec![0i64; lines.len()];
        if let Some(percentage) = value.get("percentage").and_then(Value::as_i64) {
            for &index in &entitled {
                amounts[index] = share(line_total(&lines[index]), percentage.clamp(0, 100), 100);
            }
        } else {
            let amount = cents(value.get("amount"));
            if value.get("appliesOnEachItem").and_then(Value::as_bool) == Some(true) {
                for &index in &entitled {
                    let quantity = lines[index]
                        .get("quantity")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    let unit = cents(lines[index].get("currentPrice"));
                    amounts[index] = amount.min(unit).saturating_mul(quantity);
                }
            } else {
                let whole: i64 = entitled
                    .iter()
                    .map(|&index| line_total(&lines[index]))
                    .sum();
                let amount = amount.min(whole).max(0);
                let mut left = amount;
                for (position, &index) in entitled.iter().enumerate() {
                    let part = if position + 1 == entitled.len() {
                        left
                    } else {
                        share(amount, line_total(&lines[index]), whole)
                    };
                    amounts[index] = part;
                    left -= part;
                }
            }
        }
        for &index in &entitled {
            if amounts[index] > 0 {
                taken[index].push(json!({
                    "amount": money(amounts[index], &currency),
                    "discount": allocated(discount),
                }));
            }
        }
    }
    for (index, line) in lines.iter_mut().enumerate() {
        let off: i64 = taken[index]
            .iter()
            .map(|allocation| cents(allocation.get("amount")))
            .sum();
        let discounted = (line_total(line) - off).max(0);
        line["discountAllocations"] = Value::Array(std::mem::take(&mut taken[index]));
        line["lineDiscountedPrice"] = money(discounted, &currency);
    }
    let discounts: Vec<Value> = discounts
        .into_iter()
        .map(|mut discount| {
            if discount.get("entitledLines").is_some() {
                let entitles = entitles(&discount);
                let entitled: Vec<Value> = lines
                    .iter()
                    .filter(|line| entitles(line))
                    .map(|line| {
                        let mut line = line.clone();
                        line["discountAllocations"] = json!([]);
                        line
                    })
                    .collect();
                discount["entitledLines"]["lines"] = Value::Array(entitled);
            }
            discount
        })
        .collect();
    record.insert("lines".into(), Value::Array(lines));
    record.insert("discounts".into(), Value::Array(discounts));
    if text(record, "__typename") == Some(CONTRACT) {
        let totals = projected_totals(state, record);
        record.insert("latestCommittedProjectedOrderTotals".into(), totals);
    }
}

/// What a contract-like record's next order costs, as `SubscriptionContractCalculation
/// ProjectedOrderTotals` says it: the lines after their discounts, the delivery and its
/// discounts, the tax at the store data's rate (on top, or included as the shop says), the
/// total.
pub fn projected_totals(state: &ShopState, record: &Record) -> Value {
    let (subtotal, merchandise_discounts, delivery, delivery_discounts, tax, total) =
        amounts(state, record);
    let currency = text(record, "currencyCode").unwrap_or("USD");
    json!({
        "subtotal": money(subtotal, currency),
        "totalMerchandiseDiscounts": money(merchandise_discounts, currency),
        "totalDelivery": money(delivery, currency),
        "totalDeliveryDiscounts": money(delivery_discounts, currency),
        "totalTax": money(tax, currency),
        "total": money(total, currency),
    })
}

/// The amounts of a contract-like record's next order, in cents: the subtotal (lines after
/// their discounts), the merchandise discounts, the delivery price, the delivery discounts, the
/// tax and the total.
pub fn amounts(state: &ShopState, record: &Record) -> (i64, i64, i64, i64, i64, i64) {
    let lines = array(record, "lines");
    let before: i64 = lines.iter().map(line_total).sum();
    let subtotal: i64 = lines
        .iter()
        .map(|line| cents(line.get("lineDiscountedPrice")))
        .sum();
    let taxable: i64 = lines
        .iter()
        .filter(|line| line.get("taxable").and_then(Value::as_bool) != Some(false))
        .map(|line| cents(line.get("lineDiscountedPrice")))
        .sum();
    let delivery = cents(record.get("deliveryPrice"));
    let mut delivery_discounts = 0;
    for discount in array(record, "discounts")
        .iter()
        .filter(|discount| applies(discount))
    {
        if discount.get("targetType").and_then(Value::as_str) != Some("SHIPPING_LINE") {
            continue;
        }
        let value = &discount["value"];
        let off = match value.get("percentage").and_then(Value::as_i64) {
            Some(percentage) => share(delivery, percentage.clamp(0, 100), 100),
            None => cents(value.get("amount")),
        };
        delivery_discounts = (delivery_discounts + off).min(delivery);
    }
    let rate = state
        .setting(TAX_RATE_SETTING)
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let included = state
        .shop()
        .and_then(|shop| shop.get("taxesIncluded"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tax_exempt = record
        .get("customer")
        .and_then(Value::as_str)
        .and_then(|customer| state.get(customer))
        .and_then(|customer| customer.get("taxExempt"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tax = if rate <= 0 || tax_exempt {
        0
    } else if included {
        ((taxable as f64) * rate as f64 / (10_000.0 + rate as f64)).round() as i64
    } else {
        share(taxable, rate, 10_000)
    };
    let total = subtotal + delivery - delivery_discounts + if included { 0 } else { tax };
    (
        subtotal,
        before - subtotal,
        delivery,
        delivery_discounts,
        tax,
        total,
    )
}

/// Whether a line of a contract-like record ships.
pub fn requires_shipping(record: &Record) -> bool {
    array(record, "lines")
        .iter()
        .any(|line| line.get("requiresShipping").and_then(Value::as_bool) != Some(false))
}

/// The errors of a contract-like record about to become a contract (`path` the input the
/// errors are reported at): its lines, its delivery method, its discounts.
pub fn check_committable(record: &Record, path: &[&str], errors: &mut Vec<UserError>) {
    let count = array(record, "lines").len();
    if count == 0 {
        errors.push(draft_error(&input_path(path, &["lines"]), "INVALID_LINES"));
    } else if count > MAX_LINES {
        errors.push(draft_error(&input_path(path, &["lines"]), "TOO_MANY_LINES"));
    }
    if array(record, "discounts").len() > MAX_DISCOUNTS {
        errors.push(draft_error(
            &input_path(path, &["discounts"]),
            "TOO_MANY_DISCOUNTS",
        ));
    }
    if requires_shipping(record) && record.get("deliveryMethod").is_none_or(Value::is_null) {
        errors.push(draft_error(
            &input_path(path, &["deliveryMethod"]),
            "DELIVERY_METHOD_REQUIRED",
        ));
    }
    if record.get("billingPolicy").is_none_or(Value::is_null) {
        errors.push(draft_error(
            &input_path(path, &["billingPolicy"]),
            "PRESENCE",
        ));
    }
    if record.get("deliveryPolicy").is_none_or(Value::is_null) {
        errors.push(draft_error(
            &input_path(path, &["deliveryPolicy"]),
            "PRESENCE",
        ));
    }
}

/// The note of an input, checked (`INVALID_NOTE_LENGTH`).
pub fn check_note(input: &Map<String, Value>, path: &[&str], errors: &mut Vec<UserError>) {
    if text(input, "note").is_some_and(|note| note.chars().count() > MAX_NOTE) {
        errors.push(draft_error(
            &input_path(path, &["note"]),
            "INVALID_NOTE_LENGTH",
        ));
    }
}

// ---- payment methods ------------------------------------------------------------------------

/// The payment method of `id` for a contract of `customer`, or the error of its input field.
pub fn check_payment_method(
    state: &ShopState,
    id: &str,
    customer: Option<&str>,
    field: &[&str],
) -> Option<UserError> {
    let Some(method) = state
        .get(id)
        .filter(|_| state.type_of(id) == Some(PAYMENT_METHOD))
    else {
        return Some(invalid_error(
            field,
            "Customer payment method does not exist",
        ));
    };
    if method
        .get("revokedAt")
        .is_some_and(|revoked| !revoked.is_null())
    {
        // lsf's wording: a revoked method cannot pay for a contract.
        return Some(invalid_error(
            field,
            "Customer payment method has been revoked",
        ));
    }
    if customer.is_some() && text(method, "customer") != customer {
        return Some(draft_error(field, "CUSTOMER_MISMATCH"));
    }
    None
}

/// The contract-level fields of the draft API's `SubscriptionDraftInput` applied to a
/// contract-like record, checked (`path` the input's): policies, delivery method and price,
/// payment method, note, custom attributes, next billing date, status.
pub fn apply_draft_input(
    state: &mut ShopState,
    record: &mut Record,
    input: &Map<String, Value>,
    path: &[&str],
    errors: &mut Vec<UserError>,
) {
    let currency = text(record, "currencyCode").unwrap_or("USD").to_string();
    let billing = input
        .get("billingPolicy")
        .filter(|value| !value.is_null())
        .map(|policy| billing_policy(&object(Some(policy))));
    let delivery = input
        .get("deliveryPolicy")
        .filter(|value| !value.is_null())
        .map(|policy| delivery_policy(&object(Some(policy))));
    check_policies(
        billing.as_ref().or(record.get("billingPolicy")),
        delivery.as_ref().or(record.get("deliveryPolicy")),
        path,
        errors,
    );
    if let Some(billing) = billing {
        record.insert("billingPolicy".into(), billing);
    }
    if let Some(delivery) = delivery {
        record.insert("deliveryPolicy".into(), delivery);
    }
    if let Some(method) = input.get("deliveryMethod") {
        if method.is_null() {
            record.insert("deliveryMethod".into(), Value::Null);
        } else {
            match delivery_method(
                state,
                &object(Some(method)),
                &input_path(path, &["deliveryMethod"]),
            ) {
                Ok(method) => {
                    record.insert("deliveryMethod".into(), method);
                }
                Err(more) => errors.extend(more),
            }
        }
    }
    if input
        .get("deliveryPrice")
        .is_some_and(|value| !value.is_null())
    {
        match decimal_cents(input.get("deliveryPrice")) {
            Some(price) if price >= 0 && value::money_in_bounds(price) => {
                record.insert("deliveryPrice".into(), money(price, &currency));
            }
            _ => errors.push(draft_error(
                &input_path(path, &["deliveryPrice"]),
                "GREATER_THAN_OR_EQUAL_TO",
            )),
        }
    }
    if let Some(method) = input.get("paymentMethodId") {
        match method.as_str() {
            Some(method) => {
                let customer = text(record, "customer").map(str::to_string);
                match check_payment_method(
                    state,
                    method,
                    customer.as_deref(),
                    &input_path(path, &["paymentMethodId"]),
                ) {
                    Some(error) => errors.push(error),
                    None => {
                        record.insert("customerPaymentMethod".into(), json!(method));
                    }
                }
            }
            None => {
                record.insert("customerPaymentMethod".into(), Value::Null);
            }
        }
    }
    check_note(input, path, errors);
    if let Some(note) = input.get("note") {
        record.insert("note".into(), note.clone());
    }
    if let Some(attributes) = input.get("customAttributes") {
        record.insert(
            "customAttributes".into(),
            crate::domains::orders::model::attributes(Some(attributes)),
        );
    }
    if let Some(date) = input
        .get("nextBillingDate")
        .filter(|value| !value.is_null())
    {
        match date.as_str().and_then(date_time) {
            Some(date) => {
                record.insert("nextBillingDate".into(), json!(value::date_time(date)));
            }
            None => errors.push(draft_error(
                &input_path(path, &["nextBillingDate"]),
                "INVALID_BILLING_DATE",
            )),
        }
    }
    if let Some(status) = input.get("status").filter(|value| !value.is_null()) {
        record.insert("status".into(), status.clone());
    }
}

/// Increments a contract's `revisionId` and sets its `updatedAt`: every committed change makes a
/// new revision, which a draft or a calculation made from an older one finds stale.
pub fn bump(state: &mut ShopState, contract: &str) {
    let now = state.timestamp();
    if let Some(record) = state.get_mut(contract) {
        let revision = record
            .get("revisionId")
            .and_then(Value::as_str)
            .and_then(|text| text.parse::<u64>().ok())
            .unwrap_or(0);
        record.insert("revisionId".into(), json!((revision + 1).to_string()));
        record.insert("updatedAt".into(), json!(now));
    }
}

/// The revision of a contract.
pub fn revision(state: &ShopState, contract: &str) -> String {
    state
        .get(contract)
        .and_then(|record| text(record, "revisionId"))
        .unwrap_or("0")
        .to_string()
}
