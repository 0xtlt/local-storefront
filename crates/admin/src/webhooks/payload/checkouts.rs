//! The payloads of the storefront's carts and checkouts: `carts/create`, `carts/update`,
//! `checkouts/create`, `checkouts/update`, `checkouts/delete`, in the keys and the order of
//! shopify.dev's examples (2026-10). A cart is built from its event (`domains::checkouts`
//! keeps no record of it), a checkout from its `AbandonedCheckout` record.

use serde_json::{Map, Value, json};

use super::customers::{embedded, order_address};
use super::{
    Lookup, bag_amount, field, grams, millis, numeric, price, price_or_zero, resolve, text, time,
};

/// A price in the cart's currency as a `MoneyBag`, as REST writes `*_set`.
fn set(amount: &Value, currency: &Value) -> Value {
    let money = json!({"amount": price_or_zero(amount), "currency_code": currency});
    json!({"shop_money": money.clone(), "presentment_money": money})
}

/// `properties`: `null` without any, else `{name: value}`.
fn properties(attributes: &Value) -> Value {
    let pairs: Map<String, Value> = attributes
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pair| {
            Some((
                pair.get("key")?.as_str()?.to_string(),
                pair.get("value").cloned().unwrap_or(Value::Null),
            ))
        })
        .collect();
    if pairs.is_empty() {
        Value::Null
    } else {
        Value::Object(pairs)
    }
}

/// `carts/create` and `carts/update`.
pub(super) fn cart(record: &Map<String, Value>) -> Value {
    let currency = field(record, "currencyCode").clone();
    let lines: Vec<Value> = field(record, "lineItems")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|line| {
            let price = field(line, "price");
            let line_price = field(line, "linePrice");
            json!({
                "id": field(line, "id"),
                "properties": properties(field(line, "properties")),
                "quantity": field(line, "quantity"),
                "variant_id": field(line, "id"),
                "key": field(line, "key"),
                "discounted_price": price_or_zero(price),
                "discounts": [],
                "gift_card": field(line, "giftCard").as_bool().unwrap_or(false),
                "grams": grams(field(line, "weight")),
                "line_price": price_or_zero(line_price),
                "original_line_price": price_or_zero(line_price),
                "original_price": price_or_zero(price),
                "price": price_or_zero(price),
                "product_id": numeric(field(line, "product")),
                "sku": text(line, "sku"),
                "taxable": field(line, "taxable").as_bool().unwrap_or(true),
                "title": text(line, "title"),
                "total_discount": "0.00",
                "vendor": text(line, "vendor"),
                "discounted_price_set": set(price, &currency),
                "line_price_set": set(line_price, &currency),
                "original_line_price_set": set(line_price, &currency),
                "price_set": set(price, &currency),
                "total_discount_set": set(&json!("0.00"), &currency),
                "parent_relationship": null,
            })
        })
        .collect();
    let note = match field(record, "note") {
        Value::String(note) if !note.is_empty() => json!(note),
        _ => Value::Null,
    };
    json!({
        "id": text(record, "token"),
        "token": text(record, "token"),
        "line_items": lines,
        "note": note,
        "updated_at": millis(field(record, "updatedAt")),
        "created_at": millis(field(record, "createdAt")),
    })
}

/// The tax rate of a checkout: its first tax line's.
fn rate(record: &Map<String, Value>) -> Value {
    field(record, "taxLines")
        .as_array()
        .and_then(|lines| lines.first())
        .and_then(|line| line.get("rate"))
        .cloned()
        .unwrap_or(json!(0.0))
}

/// The cents of an amount.
fn cents(amount: &Value) -> i64 {
    crate::domains::catalog::cents_of(Some(amount)).unwrap_or(0)
}

/// A line item of a checkout, its share of the checkout's tax by its total.
fn checkout_line(
    line: &Map<String, Value>,
    tax: (i64, i64),
    rate: &Value,
    lookup: &dyn Lookup,
) -> Value {
    let (total_tax, total_taxable) = tax;
    let line_total = cents(&bag_amount(field(line, "discountedTotalPriceSet")));
    let share = if total_taxable > 0 {
        (i128::from(total_tax) * i128::from(line_total) / i128::from(total_taxable)) as i64
    } else {
        0
    };
    let variant = resolve(field(line, "variant"), lookup).unwrap_or_default();
    let weight = resolve(field(&variant, "inventoryItem"), lookup)
        .and_then(|item| {
            item.get("measurement")
                .and_then(|m| m.get("weight"))
                .cloned()
        })
        .unwrap_or(Value::Null);
    let quantity = field(line, "quantity").as_i64().unwrap_or(1).max(1);
    let tax_price = crate::state::value::money(share);
    json!({
        "applied_discounts": [],
        "discount_allocations": [],
        "key": match text(line, "lsfKey") {
            Value::Null => text(line, "id"),
            key => key,
        },
        "destination_location_id": null,
        "fulfillment_service": "manual",
        "gift_card": false,
        "grams": grams(&weight).as_i64().map_or(json!(0), |unit| json!(unit)),
        "origin_location_id": null,
        "presentment_title": text(line, "title"),
        "presentment_variant_title": text(line, "variantTitle"),
        "product_id": numeric(field(line, "product")),
        "properties": properties(field(line, "customAttributes")),
        "quantity": field(line, "quantity"),
        "requires_shipping": field(line, "requiresShipping").as_bool().unwrap_or(true),
        "sku": text(line, "sku"),
        "tax_lines": if share > 0 {
            json!([{
                "price": tax_price,
                "position": 1,
                "rate": rate,
                "title": "Tax",
                "source": "Shopify",
                "zone": null,
                "jurisdiction_id": null,
                "jurisdiction_type": null,
                "jurisdiction_source": null,
                "reporting_taxable_amount": null,
                "reporting_non_taxable_amount": null,
                "reporting_exempt_amount": null,
                "reporting_jurisdiction_name": null,
                "reporting_jurisdiction_type": null,
                "reporting_jurisdiction_code": null,
                "tax_api_client_id": null,
                "tax_calculation_price": tax_price,
                "tax_registration_id": null,
                "compare_at": rate,
                "channel_liable": false,
            }])
        } else {
            json!([])
        },
        "taxable": field(&variant, "taxable").as_bool().unwrap_or(true),
        "title": text(line, "title"),
        "variant_id": numeric(field(line, "variant")),
        "variant_title": text(line, "variantTitle"),
        "variant_price": price(&bag_amount(field(line, "originalUnitPriceSet"))),
        "vendor": text(line, "vendor"),
        "user_id": null,
        "unit_price_measurement": {
            "measured_type": null,
            "quantity_value": null,
            "quantity_unit": null,
            "reference_value": null,
            "reference_unit": null,
        },
        "rank": null,
        "compare_at_price": price(field(&variant, "compareAtPrice")),
        "line_price": price(&bag_amount(field(line, "originalTotalPriceSet"))),
        "price": price(&json!(crate::state::value::money(
            cents(&bag_amount(field(line, "originalTotalPriceSet"))) / quantity
        ))),
    })
}

/// `checkouts/create` and `checkouts/update`.
pub(super) fn checkout(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let lines: Vec<Map<String, Value>> = field(record, "lineItems")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .cloned()
        .collect();
    let total_tax = cents(&bag_amount(field(record, "totalTaxSet")));
    let taxable: i64 = lines
        .iter()
        .map(|line| cents(&bag_amount(field(line, "discountedTotalPriceSet"))))
        .sum();
    let rate = rate(record);
    let line_items: Vec<Value> = lines
        .iter()
        .map(|line| checkout_line(line, (total_tax, taxable), &rate, lookup))
        .collect();
    let weight: i64 = line_items
        .iter()
        .map(|line| {
            line.get("grams").and_then(Value::as_i64).unwrap_or(0)
                * line.get("quantity").and_then(Value::as_i64).unwrap_or(0)
        })
        .sum();
    let currency = field(record, "lsfCurrency").clone();
    let shipping_lines: Vec<Value> = field(record, "lsfShippingLines")
        .as_array()
        .into_iter()
        .flatten()
        .map(|line| {
            json!({
                "code": line.get("handle"),
                "price": price_or_zero(line.get("price").unwrap_or(&Value::Null)),
                "title": line.get("title"),
                "source": "shopify",
            })
        })
        .collect();
    let customer = resolve(field(record, "customer"), lookup);
    let customer_value = customer
        .as_ref()
        .map_or(Value::Null, |customer| embedded(customer, lookup));
    let accepts_marketing = customer
        .as_ref()
        .and_then(|customer| customer.get("emailMarketingConsent"))
        .and_then(|consent| consent.get("marketingState"))
        .and_then(Value::as_str)
        == Some("SUBSCRIBED");
    let accepts_sms = customer
        .as_ref()
        .and_then(|customer| customer.get("smsMarketingConsent"))
        .and_then(|consent| consent.get("marketingState"))
        .and_then(Value::as_str)
        == Some("SUBSCRIBED");
    let discount_codes: Vec<Value> = field(record, "discountCodes")
        .as_array()
        .into_iter()
        .flatten()
        .map(|code| json!({"code": code, "amount": price_or_zero(&bag_amount(field(record, "totalDiscountSet"))), "type": "fixed_amount"}))
        .collect();
    let tax_lines: Vec<Value> = field(record, "taxLines")
        .as_array()
        .into_iter()
        .flatten()
        .map(|line| {
            json!({
                "price": price_or_zero(&bag_amount(line.get("priceSet").unwrap_or(&Value::Null))),
                "rate": line.get("rate").cloned().unwrap_or(json!(0.0)),
                "title": line.get("title").cloned().unwrap_or(json!("Tax")),
                "channel_liable": line.get("channelLiable").cloned().unwrap_or(json!(false)),
            })
        })
        .collect();
    let address = |name: &str| order_address(field(record, name).as_object());
    let note = match field(record, "note") {
        Value::String(note) if !note.is_empty() => json!(note),
        _ => Value::Null,
    };
    let note_attributes: Vec<Value> = field(record, "customAttributes")
        .as_array()
        .into_iter()
        .flatten()
        .map(|pair| json!({"name": pair.get("key"), "value": pair.get("value")}))
        .collect();
    let head = json!({
        "token": text(record, "lsfToken"),
        "cart_token": text(record, "lsfCartToken"),
        "email": text(record, "lsfEmail"),
        "gateway": text(record, "lsfGateway"),
        "buyer_accepts_marketing": accepts_marketing,
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "landing_site": null,
        "note": note,
        "note_attributes": note_attributes,
        "referring_site": null,
        "shipping_lines": shipping_lines,
        "taxes_included": field(record, "taxesIncluded").as_bool().unwrap_or(false),
        "total_weight": weight,
        "currency": currency,
        "completed_at": time(field(record, "completedAt")),
        "closed_at": null,
        "user_id": null,
        "location_id": null,
        "source_identifier": null,
        "source_url": null,
        "device_id": null,
        "phone": text(record, "lsfPhone"),
        "customer_locale": text(record, "lsfLocale"),
    });
    let rest = json!({
        "line_items": line_items,
        "name": text(record, "name"),
        "source": null,
        "abandoned_checkout_url": text(record, "abandonedCheckoutUrl"),
        "discount_codes": discount_codes,
        "tax_lines": tax_lines,
        "source_name": "web",
        "presentment_currency": currency,
        "buyer_accepts_sms_marketing": accepts_sms,
        "sms_marketing_phone": null,
        "total_discounts": price_or_zero(&bag_amount(field(record, "totalDiscountSet"))),
        "total_line_items_price": price_or_zero(&bag_amount(field(record, "totalLineItemsPriceSet"))),
        "total_price": price_or_zero(&bag_amount(field(record, "totalPriceSet"))),
        "total_tax": price_or_zero(&bag_amount(field(record, "totalTaxSet"))),
        "subtotal_price": price_or_zero(&bag_amount(field(record, "subtotalPriceSet"))),
        "total_duties": null,
        "reservation_token": null,
        "billing_address": address("billingAddress"),
        "shipping_address": address("shippingAddress"),
        "customer": customer_value,
    });
    super::merge([head, rest])
}
