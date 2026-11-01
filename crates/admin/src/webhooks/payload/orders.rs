//! The payloads of orders: orders and their line items, shipping lines, discounts and taxes, draft
//! orders, fulfillments, refunds and their transactions.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::customers::{embedded, order_address};
use super::{
    Lookup, amount_set, attributes, bag_amount, bag_currency, boolean, decimal_text, field, first,
    grams, id, integer, item_weight, items, lower, lower_or, merge, money_amount, money_set,
    numeric, object, price, price_or_zero, records, ref_gid, ref_id, resolve, strings, tags, text,
    text_or, time, zero_set,
};

/// `orders/create`, `/updated`, `/paid`, `/cancelled`, `/fulfilled`, `/partially_fulfilled`: an
/// `Order`.
pub(super) fn order(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let order_id = id(record);
    let line_items = records(field(record, "lineItems"), lookup);
    let shipping_lines = records(field(record, "shippingLines"), lookup);
    let applications = items(field(record, "discountApplications"));
    let currency = match text(record, "currencyCode") {
        Value::Null => bag_currency(first(record, &["totalPriceSet", "currentTotalPriceSet"])),
        currency => currency,
    };
    // A `current…Set` and the original one are the same until the order is edited or refunded:
    // each stands in for the other.
    let total_price = first(record, &["totalPriceSet", "currentTotalPriceSet"]);
    let current_total_price = first(record, &["currentTotalPriceSet", "totalPriceSet"]);
    let subtotal = first(record, &["subtotalPriceSet", "currentSubtotalPriceSet"]);
    let current_subtotal = first(record, &["currentSubtotalPriceSet", "subtotalPriceSet"]);
    let discounts = first(record, &["totalDiscountsSet", "currentTotalDiscountsSet"]);
    let current_discounts = first(record, &["currentTotalDiscountsSet", "totalDiscountsSet"]);
    let tax = first(record, &["totalTaxSet", "currentTotalTaxSet"]);
    let current_tax = first(record, &["currentTotalTaxSet", "totalTaxSet"]);
    let shipping = first(
        record,
        &["totalShippingPriceSet", "currentShippingPriceSet"],
    );
    let current_shipping = first(
        record,
        &["currentShippingPriceSet", "totalShippingPriceSet"],
    );
    let line_items_price = match field(record, "totalLineItemsPriceSet") {
        Value::Null => sum_set(line_items.iter().map(line_original_total), &currency),
        set => money_set(set),
    };
    let or_zero = |set: Value| {
        if set.is_null() {
            zero_set(&currency)
        } else {
            set
        }
    };
    let cash_rounding = field(record, "totalCashRoundingAdjustment");
    let number = integer(record, "number", Value::Null);
    let customer = resolve(field(record, "customer"), lookup);
    let fulfillments: Vec<Value> = records(field(record, "fulfillments"), lookup)
        .iter()
        .map(|fulfillment| self::fulfillment(fulfillment, Some(record), lookup))
        .collect();
    let refunds: Vec<Value> = records(field(record, "refunds"), lookup)
        .iter()
        .map(|refund| self::refund(refund, lookup))
        .collect();
    // In chunks: one `json!` of every field is deeper than the macro may go.
    merge([
        json!({
            "id": order_id,
            "admin_graphql_api_id": text(record, "id"),
            "app_id": ref_id(field(record, "app")),
            "browser_ip": text(record, "clientIp"),
            "buyer_accepts_marketing": boolean(record, "customerAcceptsMarketing", false),
            "cancel_reason": lower(field(record, "cancelReason")),
            "cancelled_at": time(field(record, "cancelledAt")),
            "cart_token": text(record, "cartToken"),
            "checkout_token": text(record, "checkoutToken"),
            "client_details": Value::Null,
            "closed_at": time(field(record, "closedAt")),
            "confirmation_number": text(record, "confirmationNumber"),
            "confirmed": boolean(record, "confirmed", true),
            "contact_email": text(record, "email"),
            "created_at": time(field(record, "createdAt")),
            "currency": currency,
            "current_shipping_price_set": or_zero(money_set(current_shipping)),
            "current_subtotal_price": price_or_zero(&bag_amount(current_subtotal)),
            "current_subtotal_price_set": or_zero(money_set(current_subtotal)),
            "current_total_additional_fees_set": money_set(field(record, "currentTotalAdditionalFeesSet")),
            "current_total_discounts": price_or_zero(&bag_amount(current_discounts)),
            "current_total_discounts_set": or_zero(money_set(current_discounts)),
            "current_total_duties_set": money_set(field(record, "currentTotalDutiesSet")),
            "current_total_price": price_or_zero(&bag_amount(current_total_price)),
            "current_total_price_set": or_zero(money_set(current_total_price)),
            "current_total_tax": price_or_zero(&bag_amount(current_tax)),
            "current_total_tax_set": or_zero(money_set(current_tax)),
        }),
        json!({
            "customer_locale": text(record, "customerLocale"),
            "device_id": Value::Null,
            "discount_codes": discount_codes(record, &applications, &line_items, &shipping_lines),
            "duties_included": boolean(record, "dutiesIncluded", false),
            "email": text(record, "email"),
            "estimated_taxes": boolean(record, "estimatedTaxes", false),
            "financial_status": lower_or(field(record, "displayFinancialStatus"), "pending"),
            "fulfillment_status": order_fulfillment_status(field(record, "displayFulfillmentStatus")),
            "landing_site": Value::Null,
            "landing_site_ref": Value::Null,
            "location_id": ref_id(first(record, &["retailLocation", "physicalLocation"])),
            "merchant_business_entity_id": Value::Null,
            "merchant_of_record_app_id": ref_id(field(record, "merchantOfRecordApp")),
            "name": text(record, "name"),
            "note": text(record, "note"),
            "note_attributes": attributes(field(record, "customAttributes")),
            // GraphQL's `number` is REST's `order_number` (from 1001); REST's `number` counts from 1.
            "number": number.as_i64().map_or(Value::Null, |number| json!(if number > 1000 { number - 1000 } else { number })),
        }),
        json!({
            "order_number": number,
            "order_status_url": text(record, "statusPageUrl"),
            "original_total_additional_fees_set": money_set(field(record, "originalTotalAdditionalFeesSet")),
            "original_total_duties_set": money_set(field(record, "originalTotalDutiesSet")),
            "payment_gateway_names": payment_gateway_names(record, lookup),
            "phone": text(record, "phone"),
            "po_number": text(record, "poNumber"),
            "presentment_currency": match text(record, "presentmentCurrencyCode") {
                Value::Null => currency.clone(),
                code => code,
            },
            "processed_at": time(first(record, &["processedAt", "createdAt"])),
            "reference": Value::Null,
            "referring_site": Value::Null,
            "source_identifier": text(record, "sourceIdentifier"),
            "source_name": text_or(record, "sourceName", "web"),
            "source_url": Value::Null,
            "subtotal_price": price_or_zero(&bag_amount(subtotal)),
            "subtotal_price_set": or_zero(money_set(subtotal)),
            "tags": tags(field(record, "tags")),
            "tax_exempt": boolean(record, "taxExempt", false),
            "tax_lines": tax_lines(field(record, "taxLines")),
            "taxes_included": boolean(record, "taxesIncluded", false),
            "test": boolean(record, "test", false),
            "token": token(record),
        }),
        json!({
            "total_cash_rounding_payment_adjustment_set": or_zero(money_set(cash_rounding.get("paymentSet").unwrap_or(&Value::Null))),
            "total_cash_rounding_refund_adjustment_set": or_zero(money_set(cash_rounding.get("refundSet").unwrap_or(&Value::Null))),
            "total_discounts": price_or_zero(&bag_amount(discounts)),
            "total_discounts_set": or_zero(money_set(discounts)),
            "total_line_items_price": line_items_price
                .pointer("/shop_money/amount")
                .cloned()
                .unwrap_or_else(|| json!("0.00")),
            "total_line_items_price_set": line_items_price,
            "total_outstanding": price_or_zero(&bag_amount(field(record, "totalOutstandingSet"))),
            "total_price": price_or_zero(&bag_amount(total_price)),
            "total_price_set": or_zero(money_set(total_price)),
            "total_shipping_price_set": or_zero(money_set(shipping)),
            "total_tax": price_or_zero(&bag_amount(tax)),
            "total_tax_set": or_zero(money_set(tax)),
            "total_tip_received": price_or_zero(&bag_amount(field(record, "totalTipReceivedSet"))),
            "total_weight": integer(record, "totalWeight", json!(0)),
            "updated_at": time(field(record, "updatedAt")),
            "user_id": ref_id(field(record, "staffMember")),
        }),
        json!({
            "billing_address": order_address(resolve(field(record, "billingAddress"), lookup).as_ref()),
            "customer": customer.as_ref().map_or(Value::Null, |customer| embedded(customer, lookup)),
            "discount_applications": applications.iter().map(discount_application).collect::<Vec<_>>(),
            "fulfillments": fulfillments,
            "line_items": line_items.iter().map(|line| line_item(line, lookup)).collect::<Vec<_>>(),
            "payment_terms": payment_terms(field(record, "paymentTerms"), record, "Order", lookup),
            "refunds": refunds,
            "shipping_address": order_address(resolve(field(record, "shippingAddress"), lookup).as_ref()),
            "shipping_lines": shipping_lines.iter().map(shipping_line).collect::<Vec<_>>(),
            "returns": [],
            "line_item_groups": [],
        }),
    ])
}

/// REST's `token` of an order: the one in its status page URL
/// (`…/orders/<token>/authenticate?key=…`), else one derived from its GID, stable across runs.
fn token(record: &Map<String, Value>) -> Value {
    if let Some(url) = field(record, "statusPageUrl").as_str()
        && let Some((_, rest)) = url.split_once("/orders/")
        && let Some(token) = rest.split(['/', '?']).next()
        && !token.is_empty()
    {
        return json!(token);
    }
    match field(record, "id").as_str() {
        Some(gid) => {
            let digest = Sha256::digest(gid.as_bytes());
            json!(
                digest[..16]
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            )
        }
        None => Value::Null,
    }
}

/// `displayFulfillmentStatus` as REST's `fulfillment_status`: `null` until something shipped.
fn order_fulfillment_status(status: &Value) -> Value {
    match status.as_str() {
        Some("FULFILLED") => json!("fulfilled"),
        Some("PARTIALLY_FULFILLED") => json!("partial"),
        Some("RESTOCKED") => json!("restocked"),
        _ => Value::Null,
    }
}

/// A line item of an order, in the order payload's key order.
fn line_item(line: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let parts = LineParts::of(line, lookup);
    json!({
        "id": id(line),
        "admin_graphql_api_id": text(line, "id"),
        "attributed_staffs": match field(line, "staffMember") {
            Value::Null => json!([]),
            staff => json!([{ "id": ref_gid(staff), "quantity": parts.quantity }]),
        },
        "current_quantity": integer(line, "currentQuantity", parts.quantity.clone()),
        "fulfillable_quantity": parts.fulfillable,
        "fulfillment_service": "manual",
        "fulfillment_status": parts.fulfillment_status,
        "gift_card": parts.gift_card,
        "grams": parts.grams,
        "name": text(line, "name"),
        "price": parts.price,
        "price_set": parts.price_set,
        "product_exists": parts.product_exists,
        "product_id": parts.product_id,
        "properties": attributes(field(line, "customAttributes")),
        "quantity": parts.quantity,
        "requires_shipping": parts.requires_shipping,
        "sales_line_item_group_id": ref_id(field(line, "lineItemGroup")),
        "selling_plan_id": match field(line, "sellingPlan").get("sellingPlanId") {
            Some(gid) => numeric(gid),
            None => Value::Null,
        },
        "sku": text(line, "sku"),
        "taxable": parts.taxable,
        "title": text(line, "title"),
        "total_discount": parts.total_discount,
        "total_discount_set": parts.total_discount_set,
        "variant_id": parts.variant_id,
        "variant_inventory_management": parts.inventory_management,
        "variant_title": text(line, "variantTitle"),
        "vendor": text(line, "vendor"),
        "tax_lines": tax_lines(field(line, "taxLines")),
        "duties": [],
        "discount_allocations": discount_allocations(field(line, "discountAllocations")),
    })
}

/// A line item as a fulfillment or a refund writes it: the same fields in another order, and the
/// quantity of the fulfillment rather than of the order.
fn line_item_of_fulfillment(
    line: &Map<String, Value>,
    quantity: Option<Value>,
    lookup: &dyn Lookup,
) -> Value {
    let parts = LineParts::of(line, lookup);
    json!({
        "id": id(line),
        "variant_id": parts.variant_id,
        "title": text(line, "title"),
        "quantity": quantity.unwrap_or(parts.quantity),
        "sku": text(line, "sku"),
        "variant_title": text(line, "variantTitle"),
        "vendor": text(line, "vendor"),
        "fulfillment_service": "manual",
        "product_id": parts.product_id,
        "requires_shipping": parts.requires_shipping,
        "taxable": parts.taxable,
        "gift_card": parts.gift_card,
        "name": text(line, "name"),
        "variant_inventory_management": parts.inventory_management,
        "properties": attributes(field(line, "customAttributes")),
        "product_exists": parts.product_exists,
        "fulfillable_quantity": parts.fulfillable,
        "grams": parts.grams,
        "price": parts.price,
        "total_discount": parts.total_discount,
        "fulfillment_status": parts.fulfillment_status,
        "price_set": parts.price_set,
        "total_discount_set": parts.total_discount_set,
        "discount_allocations": discount_allocations(field(line, "discountAllocations")),
        "duties": [],
        "admin_graphql_api_id": text(line, "id"),
        "tax_lines": tax_lines(field(line, "taxLines")),
    })
}

/// What the two forms of a line item compute the same way.
struct LineParts {
    quantity: Value,
    fulfillable: Value,
    fulfillment_status: Value,
    gift_card: Value,
    grams: Value,
    price: Value,
    price_set: Value,
    product_exists: Value,
    product_id: Value,
    requires_shipping: Value,
    taxable: Value,
    total_discount: Value,
    total_discount_set: Value,
    variant_id: Value,
    inventory_management: Value,
}

impl LineParts {
    fn of(line: &Map<String, Value>, lookup: &dyn Lookup) -> LineParts {
        let quantity = integer(line, "quantity", json!(1));
        let unfulfilled = field(line, "unfulfilledQuantity").as_i64();
        let fulfillable = integer(
            line,
            "fulfillableQuantity",
            unfulfilled.map_or(quantity.clone(), Value::from),
        );
        let fulfillment_status = match field(line, "fulfillmentStatus").as_str() {
            Some("fulfilled") => json!("fulfilled"),
            Some("partial") => json!("partial"),
            Some(_) => Value::Null,
            None => match (unfulfilled, quantity.as_i64()) {
                (Some(0), Some(quantity)) if quantity > 0 => json!("fulfilled"),
                (Some(left), Some(quantity)) if left > 0 && left < quantity => json!("partial"),
                _ => Value::Null,
            },
        };
        let product = field(line, "product");
        let item = resolve(field(line, "variant"), lookup)
            .and_then(|variant| resolve(field(&variant, "inventoryItem"), lookup));
        let tracked = item
            .as_ref()
            .and_then(|item| field(item, "tracked").as_bool());
        let weight = match field(line, "weight") {
            Value::Null => item.as_ref().map_or(Value::Null, item_weight),
            weight => weight.clone(),
        };
        let unit = first(line, &["originalUnitPriceSet", "originalUnitPrice"]);
        let discount = first(line, &["totalDiscountSet", "totalDiscount"]);
        LineParts {
            fulfillable,
            fulfillment_status,
            gift_card: boolean(line, "isGiftCard", false),
            grams: grams(&weight),
            price: price_or_zero(&set_or_money_amount(unit)),
            price_set: money_set(unit),
            product_exists: json!(match product {
                Value::Null => false,
                Value::String(gid) => lookup.record(gid).is_some(),
                _ => true,
            }),
            product_id: ref_id(product),
            requires_shipping: boolean(line, "requiresShipping", true),
            taxable: boolean(line, "taxable", true),
            total_discount: price_or_zero(&set_or_money_amount(discount)),
            total_discount_set: match money_set(discount) {
                Value::Null => zero_set(&bag_currency(unit)),
                set => set,
            },
            variant_id: ref_id(field(line, "variant")),
            inventory_management: match tracked {
                Some(true) => json!("shopify"),
                _ => Value::Null,
            },
            quantity,
        }
    }
}

/// The amount of a `MoneyBag`, or of a `Money` (a string) or `MoneyV2`.
fn set_or_money_amount(value: &Value) -> Value {
    match value {
        Value::Object(object) if object.contains_key("shopMoney") => bag_amount(value),
        Value::Object(_) => money_amount(value),
        other => price(other),
    }
}

/// What a line item costs before discounts: `originalTotalSet`, else its unit price times its
/// quantity. In hundredths, with its currency.
fn line_original_total(line: &Map<String, Value>) -> Option<(i128, Value)> {
    if let Some(total) = field(line, "originalTotalSet").get("shopMoney") {
        return Some((
            cents(total.get("amount")?)?,
            bag_currency(field(line, "originalTotalSet")),
        ));
    }
    let unit = first(line, &["originalUnitPriceSet", "originalUnitPrice"]);
    let amount = match unit {
        Value::Object(object) if object.contains_key("shopMoney") => {
            unit.pointer("/shopMoney/amount")
        }
        Value::Object(_) => unit.get("amount"),
        other => Some(other),
    }?;
    let quantity = field(line, "quantity").as_i64().unwrap_or(1);
    Some((cents(amount)? * i128::from(quantity), bag_currency(unit)))
}

/// A decimal amount in hundredths, rounded half away from zero.
fn cents(amount: &Value) -> Option<i128> {
    let text = decimal_text(amount)?;
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest.to_string()),
        None => (false, text),
    };
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let whole: i128 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let digits: Vec<u32> = fraction
        .chars()
        .map(|c| c.to_digit(10))
        .collect::<Option<_>>()?;
    let mut hundredths = whole * 100;
    hundredths += i128::from(digits.first().copied().unwrap_or(0)) * 10;
    hundredths += i128::from(digits.get(1).copied().unwrap_or(0));
    if digits.get(2).copied().unwrap_or(0) >= 5 {
        hundredths += 1;
    }
    Some(if negative { -hundredths } else { hundredths })
}

/// A sum of amounts in hundredths as a REST `MoneyBag`, in the currency of the first.
fn sum_set(amounts: impl Iterator<Item = Option<(i128, Value)>>, currency: &Value) -> Value {
    let mut total = 0i128;
    let mut code = Value::Null;
    for (amount, currency) in amounts.flatten() {
        total += amount;
        if code.is_null() {
            code = currency;
        }
    }
    if code.is_null() {
        code = currency.clone();
    }
    amount_set(&json!(decimal(total)), &code)
}

/// Cents as a decimal amount: `-150` is `-1.50`.
fn decimal(cents: i128) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    format!("{sign}{}.{:02}", cents.abs() / 100, cents.abs() % 100)
}

/// `taxLines` as REST writes them.
fn tax_lines(lines: &Value) -> Value {
    Value::Array(
        items(lines)
            .iter()
            .map(|line| {
                let line = object(line);
                let set = first(&line, &["priceSet", "price"]);
                json!({
                    "channel_liable": field(&line, "channelLiable").as_bool().unwrap_or(false),
                    "price": price_or_zero(&set_or_money_amount(set)),
                    "price_set": money_set(set),
                    "rate": field(&line, "rate"),
                    "title": text(&line, "title"),
                })
            })
            .collect(),
    )
}

/// `payment_gateway_names`: the record's `paymentGatewayNames` when it has them (a sample),
/// else the gateways of its transactions, each once, as the Admin API answers them.
fn payment_gateway_names(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    if let Some(names) = record
        .get("paymentGatewayNames")
        .filter(|names| !names.is_null())
    {
        return strings(names);
    }
    let mut names: Vec<Value> = Vec::new();
    for transaction in records(field(record, "transactions"), lookup) {
        let gateway = text(&transaction, "gateway");
        if gateway.is_string() && !names.contains(&gateway) {
            names.push(gateway);
        }
    }
    Value::Array(names)
}

/// `discountAllocations` as REST writes them, with the index of their discount application.
fn discount_allocations(allocations: &Value) -> Value {
    Value::Array(
        items(allocations)
            .iter()
            .map(|allocation| {
                let allocation = object(allocation);
                let set = first(&allocation, &["allocatedAmountSet", "allocatedAmount"]);
                json!({
                    "amount": price_or_zero(&set_or_money_amount(set)),
                    "discount_application_index": field(&allocation, "discountApplication")
                        .get("index")
                        .cloned()
                        .unwrap_or(json!(0)),
                    "amount_set": money_set(set),
                })
            })
            .collect(),
    )
}

/// A `DiscountApplication` as REST writes it.
fn discount_application(application: &Value) -> Value {
    let application = object(application);
    let kind = match field(&application, "__typename").as_str() {
        Some("DiscountCodeApplication") => "discount_code",
        Some("ManualDiscountApplication") => "manual",
        Some("ScriptDiscountApplication") => "script",
        _ => "automatic",
    };
    let value = field(&application, "value");
    let (value, value_type) = match value.get("percentage") {
        Some(percentage) => (one_decimal(percentage), "percentage"),
        None => (
            one_decimal(value.get("amount").unwrap_or(&Value::Null)),
            "fixed_amount",
        ),
    };
    let mut rest = json!({
        "target_type": match field(&application, "targetType").as_str() {
            Some("SHIPPING_LINE") => "shipping_line",
            _ => "line_item",
        },
        "type": kind,
        "value": value,
        "value_type": value_type,
        "allocation_method": lower(field(&application, "allocationMethod")),
        "target_selection": lower(field(&application, "targetSelection")),
    });
    if kind == "discount_code" {
        rest["code"] = text(&application, "code");
    } else {
        rest["title"] = text(&application, "title");
        rest["description"] = first(&application, &["description", "title"]).clone();
    }
    rest
}

/// A decimal as REST writes the `value` of a discount: `10.0`.
fn one_decimal(value: &Value) -> Value {
    match decimal_text(value) {
        Some(text) if text.contains('.') => json!(text),
        Some(text) => json!(format!("{text}.0")),
        None => Value::Null,
    }
}

/// REST's `discount_codes`: each code applied, with the amount its allocations took off.
fn discount_codes(
    record: &Map<String, Value>,
    applications: &[Value],
    line_items: &[Map<String, Value>],
    shipping_lines: &[Map<String, Value>],
) -> Value {
    let allocated = |index: &Value| -> i128 {
        line_items
            .iter()
            .chain(shipping_lines)
            .flat_map(|line| items(field(line, "discountAllocations")))
            .filter(|allocation| allocation.pointer("/discountApplication/index") == Some(index))
            .filter_map(|allocation| {
                let set = allocation
                    .get("allocatedAmountSet")
                    .or_else(|| allocation.get("allocatedAmount"))?;
                cents(
                    set.pointer("/shopMoney/amount")
                        .or_else(|| set.get("amount"))?,
                )
            })
            .sum()
    };
    let codes: Vec<Value> = applications
        .iter()
        .filter(|application| {
            application.get("__typename").and_then(Value::as_str) == Some("DiscountCodeApplication")
        })
        .map(|application| {
            let index = application.get("index").cloned().unwrap_or(Value::Null);
            let total = allocated(&index);
            let kind =
                if application.get("targetType").and_then(Value::as_str) == Some("SHIPPING_LINE") {
                    "shipping"
                } else if application.pointer("/value/percentage").is_some() {
                    "percentage"
                } else {
                    "fixed_amount"
                };
            json!({
                "code": application.get("code").cloned().unwrap_or(Value::Null),
                "amount": decimal(total),
                "type": kind,
            })
        })
        .collect();
    if !codes.is_empty() {
        return Value::Array(codes);
    }
    // The codes alone, without their applications: what they took off is unknown but for one.
    let names = strings(field(record, "discountCodes"));
    let only = names.as_array().map_or(0, Vec::len) == 1;
    Value::Array(
        names
            .as_array()
            .into_iter()
            .flatten()
            .map(|code| {
                json!({
                    "code": code,
                    "amount": if only {
                        price_or_zero(&bag_amount(first(record, &["totalDiscountsSet", "currentTotalDiscountsSet"])))
                    } else {
                        json!("0.00")
                    },
                    "type": "fixed_amount",
                })
            })
            .collect(),
    )
}

/// A `ShippingLine` as REST writes it.
fn shipping_line(line: &Map<String, Value>) -> Value {
    let original = first(line, &["originalPriceSet", "originalPrice", "price"]);
    let discounted = first(
        line,
        &["discountedPriceSet", "discountedPrice", "originalPriceSet"],
    );
    let current = first(
        line,
        &[
            "currentDiscountedPriceSet",
            "discountedPriceSet",
            "originalPriceSet",
        ],
    );
    json!({
        "id": id(line),
        "carrier_identifier": text(line, "carrierIdentifier"),
        "code": text(line, "code"),
        "current_discounted_price_set": money_set(current),
        "discounted_price": price_or_zero(&set_or_money_amount(discounted)),
        "discounted_price_set": money_set(discounted),
        "is_removed": boolean(line, "isRemoved", false),
        "phone": text(line, "phone"),
        "price": price_or_zero(&set_or_money_amount(original)),
        "price_set": money_set(original),
        "requested_fulfillment_service_id": Value::Null,
        "source": text_or(line, "source", "shopify"),
        "title": text(line, "title"),
        "tax_lines": tax_lines(field(line, "taxLines")),
        "discount_allocations": discount_allocations(field(line, "discountAllocations")),
    })
}

/// `draft_orders/create`, `draft_orders/update`: a `DraftOrder`.
pub(super) fn draft_order(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let currency = match text(record, "currencyCode") {
        Value::Null => bag_currency(field(record, "totalPriceSet")),
        currency => currency,
    };
    let customer = resolve(field(record, "customer"), lookup);
    let b2b = field(record, "purchasingEntity")
        .get("__typename")
        .and_then(Value::as_str)
        == Some("PurchasingCompany");
    json!({
        "id": id(record),
        "note": first(record, &["note2", "note"]),
        "email": text(record, "email"),
        "taxes_included": boolean(record, "taxesIncluded", false),
        "currency": currency,
        "invoice_sent_at": time(field(record, "invoiceSentAt")),
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "tax_exempt": boolean(record, "taxExempt", false),
        "completed_at": time(field(record, "completedAt")),
        "name": text(record, "name"),
        "allow_discount_codes_in_checkout?": boolean(record, "allowDiscountCodesInCheckout", false),
        "b2b?": b2b,
        "status": lower_or(field(record, "status"), "open"),
        "line_items": records(field(record, "lineItems"), lookup)
            .iter()
            .map(draft_line_item)
            .collect::<Vec<_>>(),
        "api_client_id": Value::Null,
        "shipping_address": order_address(resolve(field(record, "shippingAddress"), lookup).as_ref()),
        "billing_address": order_address(resolve(field(record, "billingAddress"), lookup).as_ref()),
        "invoice_url": text(record, "invoiceUrl"),
        "created_on_api_version_handle": Value::Null,
        "applied_discount": applied_discount(field(record, "appliedDiscount")),
        "order_id": ref_id(field(record, "order")),
        "shipping_line": match field(record, "shippingLine") {
            Value::Object(line) => json!({
                "custom": field(line, "custom").as_bool().unwrap_or(true),
                "handle": text(line, "shippingRateHandle"),
                "title": text(line, "title"),
                "price": price_or_zero(&set_or_money_amount(first(line, &["originalPriceSet", "discountedPriceSet", "price"]))),
            }),
            _ => Value::Null,
        },
        "tax_lines": draft_tax_lines(field(record, "taxLines")),
        "tags": tags(field(record, "tags")),
        "note_attributes": attributes(field(record, "customAttributes")),
        "total_price": price_or_zero(&set_or_money_amount(field(record, "totalPriceSet"))),
        "subtotal_price": price_or_zero(&set_or_money_amount(field(record, "subtotalPriceSet"))),
        "total_tax": price_or_zero(&set_or_money_amount(field(record, "totalTaxSet"))),
        "payment_terms": payment_terms(field(record, "paymentTerms"), record, "DraftOrder", lookup),
        "admin_graphql_api_id": text(record, "id"),
        "customer": customer.as_ref().map_or(Value::Null, |customer| embedded(customer, lookup)),
    })
}

/// `PaymentTerms` as REST writes them in an order's or a draft order's payload, with their
/// schedules; `reference` is the order or the draft order they are of.
fn payment_terms(
    terms: &Value,
    reference: &Map<String, Value>,
    kind: &str,
    lookup: &dyn Lookup,
) -> Value {
    let Some(terms) = resolve(terms, lookup) else {
        return Value::Null;
    };
    let terms_id = id(&terms);
    let money = |schedule: &Map<String, Value>, key: &str| {
        let money = field(schedule, key);
        (
            money_amount(money),
            money.get("currencyCode").cloned().unwrap_or(Value::Null),
        )
    };
    let schedules: Vec<Value> = records(field(&terms, "paymentSchedules"), lookup)
        .iter()
        .map(|schedule| {
            let (amount, currency) = money(schedule, "amount");
            let (balance, balance_currency) = money(schedule, "balanceDue");
            let (total, total_currency) = money(schedule, "totalBalance");
            merge([
                json!({
                    "id": id(schedule),
                    "created_at": time(first(schedule, &["createdAt", "lsfCreatedAt"])),
                    "updated_at": time(first(schedule, &["updatedAt", "lsfUpdatedAt"])),
                    "payment_terms_id": terms_id,
                    "reference_id": id(reference),
                    "reference_type": kind,
                    "issued_at": time(field(schedule, "issuedAt")),
                    "due_at": time(field(schedule, "dueAt")),
                    "completed_at": time(field(schedule, "completedAt")),
                }),
                json!({
                    "amount": amount,
                    "currency": currency,
                    "total_price": amount,
                    "total_price_currency": currency,
                    "balance_due": balance,
                    "balance_due_currency": balance_currency,
                    "total_balance": total,
                    "total_balance_currency": total_currency,
                    "outstanding_balance": balance,
                    "outstanding_balance_currency": balance_currency,
                }),
            ])
        })
        .collect();
    json!({
        "id": terms_id,
        "payment_terms_name": text(&terms, "paymentTermsName"),
        "payment_terms_type": lower(field(&terms, "paymentTermsType")),
        "due_in_days": field(&terms, "dueInDays"),
        "created_at": time(first(&terms, &["createdAt", "lsfCreatedAt"])),
        "updated_at": time(first(&terms, &["updatedAt", "lsfUpdatedAt"])),
        "payment_schedules": schedules,
        "can_pay_early": boolean(&terms, "canPayEarly", true),
    })
}

/// A `DraftOrderLineItem` as REST writes it.
fn draft_line_item(line: &Map<String, Value>) -> Value {
    let unit = first(
        line,
        &[
            "originalUnitPriceSet",
            "originalUnitPriceWithCurrency",
            "originalUnitPrice",
        ],
    );
    let weight = match field(line, "weight") {
        Value::Null => {
            json!({"unit": "GRAMS", "value": field(line, "grams").as_f64().unwrap_or(0.0)})
        }
        weight => weight.clone(),
    };
    json!({
        "id": id(line),
        "variant_id": ref_id(field(line, "variant")),
        "product_id": ref_id(field(line, "product")),
        "title": text(line, "title"),
        "variant_title": text(line, "variantTitle"),
        "sku": text(line, "sku"),
        "vendor": text(line, "vendor"),
        "quantity": integer(line, "quantity", json!(1)),
        "requires_shipping": boolean(line, "requiresShipping", true),
        "taxable": boolean(line, "taxable", true),
        "gift_card": boolean(line, "isGiftCard", false),
        "fulfillment_service": "manual",
        "grams": grams(&weight),
        "tax_lines": draft_tax_lines(field(line, "taxLines")),
        "applied_discount": applied_discount(field(line, "appliedDiscount")),
        "name": text(line, "name"),
        "properties": attributes(field(line, "customAttributes")),
        "custom": boolean(line, "custom", false),
        "price": price_or_zero(&set_or_money_amount(unit)),
        "admin_graphql_api_id": text(line, "id"),
    })
}

/// The `taxLines` of a draft order or of its line items, as REST writes them there.
fn draft_tax_lines(lines: &Value) -> Value {
    Value::Array(
        items(lines)
            .iter()
            .map(|line| {
                let line = object(line);
                json!({
                    "rate": field(&line, "rate"),
                    "title": text(&line, "title"),
                    "price": price_or_zero(&set_or_money_amount(first(&line, &["priceSet", "price"]))),
                })
            })
            .collect(),
    )
}

/// A `DraftOrderAppliedDiscount` as REST writes it.
fn applied_discount(discount: &Value) -> Value {
    let Value::Object(discount) = discount else {
        return Value::Null;
    };
    json!({
        "description": text(discount, "description"),
        "value": one_decimal(field(discount, "value")),
        "title": text(discount, "title"),
        "amount": price_or_zero(&set_or_money_amount(first(discount, &["amountSet", "amountV2", "amount"]))),
        "value_type": lower(field(discount, "valueType")),
    })
}

/// `fulfillments/create`, `fulfillments/update`: a `Fulfillment`. Its email and destination are
/// those of its order, given or looked up.
pub(super) fn fulfillment(
    record: &Map<String, Value>,
    order: Option<&Map<String, Value>>,
    lookup: &dyn Lookup,
) -> Value {
    let order = match order {
        Some(order) => Some(order.clone()),
        None => resolve(field(record, "order"), lookup),
    };
    let order_id = match order.as_ref() {
        Some(order) => id(order),
        None => ref_id(field(record, "order")),
    };
    let tracking: Vec<Map<String, Value>> = items(field(record, "trackingInfo"))
        .iter()
        .map(object)
        .collect();
    let line_items: Vec<Value> = records(field(record, "fulfillmentLineItems"), lookup)
        .iter()
        .filter_map(|fulfilled| {
            let line = resolve(field(fulfilled, "lineItem"), lookup)?;
            Some(line_item_of_fulfillment(
                &line,
                field(fulfilled, "quantity").as_i64().map(Value::from),
                lookup,
            ))
        })
        .collect();
    let shipment_status = match lower(field(record, "displayStatus")) {
        Value::String(status)
            if [
                "label_printed",
                "label_purchased",
                "attempted_delivery",
                "ready_for_pickup",
                "confirmed",
                "in_transit",
                "out_for_delivery",
                "delivered",
                "failure",
            ]
            .contains(&status.as_str()) =>
        {
            json!(status)
        }
        _ => Value::Null,
    };
    json!({
        "id": id(record),
        "order_id": order_id,
        "status": lower_or(field(record, "status"), "success"),
        "created_at": time(field(record, "createdAt")),
        "service": match field(record, "service") {
            Value::Object(service) => first(service, &["handle", "serviceName"]).clone(),
            _ => Value::Null,
        },
        "updated_at": time(field(record, "updatedAt")),
        "tracking_company": tracking.first().map_or(Value::Null, |info| text(info, "company")),
        "shipment_status": shipment_status,
        "location_id": ref_id(field(record, "location")),
        "origin_address": Value::Null,
        "email": order.as_ref().map_or(Value::Null, |order| text(order, "email")),
        "destination": order_address(
            order
                .as_ref()
                .and_then(|order| resolve(field(order, "shippingAddress"), lookup))
                .as_ref(),
        ),
        "line_items": line_items,
        "tracking_number": tracking.first().map_or(Value::Null, |info| text(info, "number")),
        "tracking_numbers": tracking.iter().map(|info| text(info, "number")).filter(|n| !n.is_null()).collect::<Vec<_>>(),
        "tracking_url": tracking.first().map_or(Value::Null, |info| text(info, "url")),
        "tracking_urls": tracking.iter().map(|info| text(info, "url")).filter(|u| !u.is_null()).collect::<Vec<_>>(),
        "receipt": {},
        "name": text(record, "name"),
        "admin_graphql_api_id": text(record, "id"),
    })
}

/// `refunds/create`: a `Refund`, with its line items and transactions.
pub(super) fn refund(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let order_id = ref_id(field(record, "order"));
    let lines = records(field(record, "refundLineItems"), lookup);
    let currency = bag_currency(field(record, "totalRefundedSet"));
    let restock = lines.iter().any(|line| {
        field(line, "restocked").as_bool() == Some(true)
            || field(line, "restockType")
                .as_str()
                .is_some_and(|kind| kind != "NO_RESTOCK")
    });
    let refund_line_items: Vec<Value> = lines
        .iter()
        .map(|line| {
            let item = resolve(field(line, "lineItem"), lookup);
            let subtotal = first(line, &["subtotalSet", "subtotal"]);
            let total_tax = first(line, &["totalTaxSet", "totalTax"]);
            let number = |value: &Value| {
                set_or_money_amount(value)
                    .as_str()
                    .and_then(|amount| amount.parse::<f64>().ok())
                    .map_or(json!(0.0), |amount| json!(amount))
            };
            json!({
                "id": id(line),
                "quantity": integer(line, "quantity", json!(1)),
                "line_item_id": item.as_ref().map_or(ref_id(field(line, "lineItem")), id),
                "location_id": ref_id(field(line, "location")),
                "restock_type": lower_or(field(line, "restockType"), "no_restock"),
                "subtotal": number(subtotal),
                "total_tax": number(total_tax),
                "subtotal_set": money_set(subtotal),
                "total_tax_set": match money_set(total_tax) {
                    Value::Null => zero_set(&bag_currency(subtotal)),
                    set => set,
                },
                "line_item": item.as_ref().map_or(Value::Null, |item| line_item_of_fulfillment(item, None, lookup)),
            })
        })
        .collect();
    json!({
        "id": id(record),
        "order_id": order_id,
        "created_at": time(first(record, &["createdAt", "processedAt"])),
        "note": text(record, "note"),
        "user_id": ref_id(field(record, "staffMember")),
        "processed_at": time(first(record, &["processedAt", "createdAt"])),
        "duties": [],
        "total_duties_set": zero_set(&currency),
        "return": ref_id(field(record, "return")),
        "refund_shipping_lines": [],
        "restock": restock,
        "admin_graphql_api_id": text(record, "id"),
        "order_adjustments": [],
        "refund_line_items": refund_line_items,
        "transactions": records(field(record, "transactions"), lookup)
            .iter()
            .map(|transaction| self::transaction(transaction, &order_id))
            .collect::<Vec<_>>(),
    })
}

/// An `OrderTransaction` as REST writes it.
fn transaction(record: &Map<String, Value>, order_id: &Value) -> Value {
    let amount = first(record, &["amountSet", "amountV2", "amount"]);
    let currency = match bag_currency(amount) {
        Value::Null => amount.get("currencyCode").cloned().unwrap_or(Value::Null),
        currency => currency,
    };
    // `total_unsettled_set` writes `currency`, not `currency_code`, and the amounts as they are.
    let unsettled = |money: Option<&Value>| match money {
        Some(money) => json!({
            "amount": decimal_text(money.get("amount").unwrap_or(&Value::Null)).unwrap_or_else(|| "0.0".to_string()),
            "currency": money.get("currencyCode").cloned().unwrap_or_else(|| json!("XXX")),
        }),
        // What Shopify writes when nothing is unsettled: no currency, `XXX`.
        None => json!({"amount": "0.0", "currency": "XXX"}),
    };
    let unsettled_set = field(record, "totalUnsettledSet");
    json!({
        "id": id(record),
        "order_id": match ref_id(field(record, "order")) {
            Value::Null => order_id.clone(),
            id => id,
        },
        "kind": lower(field(record, "kind")),
        "gateway": text(record, "gateway"),
        "status": lower(field(record, "status")),
        "message": Value::Null,
        "created_at": time(field(record, "createdAt")),
        "test": boolean(record, "test", false),
        "authorization": text(record, "authorizationCode"),
        "location_id": ref_id(field(record, "location")),
        "user_id": ref_id(field(record, "user")),
        "parent_id": ref_id(field(record, "parentTransaction")),
        "processed_at": time(field(record, "processedAt")),
        "device_id": ref_id(field(record, "device")),
        "error_code": lower(field(record, "errorCode")),
        "source_name": "web",
        "receipt": match field(record, "receiptJson") {
            Value::String(json) => serde_json::from_str(json).unwrap_or_else(|_| json!({})),
            Value::Object(receipt) => Value::Object(receipt.clone()),
            _ => json!({}),
        },
        "amount": price_or_zero(&set_or_money_amount(amount)),
        "currency": currency,
        "payment_id": text(record, "paymentId"),
        "total_unsettled_set": {
            "presentment_money": unsettled(unsettled_set.get("presentmentMoney")),
            "shop_money": unsettled(unsettled_set.get("shopMoney")),
        },
        "manual_payment_gateway": boolean(record, "manualPaymentGateway", false),
        "amount_rounding": Value::Null,
        "admin_graphql_api_id": text(record, "id"),
    })
}

/// `order_transactions/create`: an `OrderTransaction`, with its payment details.
pub(super) fn order_transaction(record: &Map<String, Value>) -> Value {
    let mut payload = match transaction(record, &Value::Null) {
        Value::Object(payload) => payload,
        _ => Map::new(),
    };
    let details = match field(record, "paymentDetails") {
        Value::Object(details) => json!({
            "credit_card_bin": text(details, "bin"),
            "avs_result_code": text(details, "avsResultCode"),
            "cvv_result_code": text(details, "cvvResultCode"),
            "credit_card_number": text(details, "number"),
            "credit_card_company": text(details, "company"),
            "buyer_action_info": Value::Null,
            "credit_card_name": text(details, "name"),
            "credit_card_wallet": text(details, "wallet"),
            "credit_card_expiration_month": field(details, "expirationMonth").clone(),
            "credit_card_expiration_year": field(details, "expirationYear").clone(),
            "payment_method_name": text(details, "paymentMethodName"),
        }),
        _ => Value::Null,
    };
    // In the key order of the example: `payment_details` after `source_name`.
    let mut ordered = Map::new();
    for (key, value) in payload.iter_mut() {
        ordered.insert(key.clone(), value.take());
        if key == "source_name" {
            ordered.insert("payment_details".into(), details.clone());
        }
    }
    Value::Object(ordered)
}

/// The fields of an order edit lsf keeps on the committed `CalculatedOrder` for
/// `orders/edited`: when it was committed, whether the customer was to be notified, the staff
/// note, and the line items added and removed (`[{id, delta}]`).
pub const EDIT_COMMITTED_AT: &str = "lsfCommittedAt";
pub const EDIT_NOTIFY_CUSTOMER: &str = "lsfNotifyCustomer";
pub const EDIT_STAFF_NOTE: &str = "lsfStaffNote";
pub const EDIT_ADDITIONS: &str = "lsfAdditions";
pub const EDIT_REMOVALS: &str = "lsfRemovals";

/// `[{id, delta}]` with REST's numeric ids.
fn deltas(value: &Value) -> Value {
    Value::Array(
        items(value)
            .iter()
            .map(|item| json!({"id": ref_id(item.get("id").unwrap_or(&Value::Null)), "delta": item.get("delta")}))
            .collect(),
    )
}

/// `orders/edited`: the edit, from its committed `CalculatedOrder`.
pub(super) fn order_edit(record: &Map<String, Value>) -> Value {
    json!({
        "order_edit": {
            "id": id(record),
            "app_id": ref_id(field(record, "app")),
            "created_at": time(field(record, "createdAt")),
            "committed_at": time(field(record, EDIT_COMMITTED_AT)),
            "notify_customer": boolean(record, EDIT_NOTIFY_CUSTOMER, false),
            "order_id": ref_id(field(record, "originalOrder")),
            "staff_note": text_or(record, EDIT_STAFF_NOTE, ""),
            "user_id": Value::Null,
            "line_items": {
                "additions": deltas(field(record, EDIT_ADDITIONS)),
                "removals": deltas(field(record, EDIT_REMOVALS)),
            },
            "discounts": {"line_item": {"additions": [], "removals": []}},
            "shipping_lines": {"additions": [], "removals": []},
            "shipping_address": {"changed": false},
        }
    })
}

/// `fulfillment_events/create`: a `FulfillmentEvent`; its fulfillment (which lsf keeps on it)
/// gives the order.
pub(super) fn fulfillment_event(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let fulfillment = resolve(field(record, "fulfillment"), lookup);
    let shop = lookup.shop();
    json!({
        "id": id(record),
        "fulfillment_id": ref_id(field(record, "fulfillment")),
        "status": lower(field(record, "status")),
        "message": text(record, "message"),
        "happened_at": time(field(record, "happenedAt")),
        "city": text(record, "city"),
        "province": text(record, "province"),
        "country": text(record, "country"),
        "zip": text(record, "zip"),
        "address1": text(record, "address1"),
        "latitude": field(record, "latitude").clone(),
        "longitude": field(record, "longitude").clone(),
        "shop_id": shop.as_ref().map_or(Value::Null, id),
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(first(record, &["updatedAt", "createdAt"])),
        "estimated_delivery_at": time(field(record, "estimatedDeliveryAt")),
        "order_id": fulfillment.as_ref().map_or(Value::Null, |fulfillment| ref_id(field(fulfillment, "order"))),
        "admin_graphql_api_id": text(record, "id"),
    })
}

/// A `FulfillmentHold` as the fulfillment order topics write it.
fn hold(record: &Map<String, Value>) -> Value {
    json!({
        "id": text(record, "id"),
        "reason": lower(field(record, "reason")),
        "reason_notes": text(record, "reasonNotes"),
        "held_by_requesting_app": boolean(record, "heldByRequestingApp", false),
        "handle": text(record, "handle"),
        "held_by_app": match ref_gid(field(record, "heldByApp")) {
            Value::Null => Value::Null,
            app => json!({"id": app}),
        },
    })
}

/// `fulfillment_holds/added`, `fulfillment_holds/released`: a `FulfillmentHold`, with the
/// fulfillment order lsf keeps on it.
pub(super) fn fulfillment_hold(record: &Map<String, Value>) -> Value {
    json!({
        "fulfillment_order": {"id": ref_gid(field(record, "fulfillmentOrder"))},
        "fulfillment_hold": hold(record),
    })
}

/// What a fulfillment order topic writes of a fulfillment order: its GID and status.
fn fulfillment_order_ref(reference: &Value, lookup: &dyn Lookup) -> Value {
    match resolve(reference, lookup) {
        Some(record) => {
            json!({"id": text(&record, "id"), "status": lower(field(&record, "status"))})
        }
        None => Value::Null,
    }
}

/// The context of a fulfillment order topic lsf keeps on the fulfillment order it is about: the
/// other fulfillment orders and line items of the change (`lsfEvent`).
pub const FULFILLMENT_ORDER_EVENT: &str = "lsfEvent";

/// `[{id, quantity}]` of fulfillment order line items.
fn line_quantities(value: &Value) -> Value {
    Value::Array(
        items(value)
            .iter()
            .map(|item| json!({"id": item.get("id"), "quantity": item.get("quantity")}))
            .collect(),
    )
}

/// `fulfillment_orders/…`: a `FulfillmentOrder`, with what lsf kept of the change.
pub(super) fn fulfillment_order_event(
    topic: &str,
    record: &Map<String, Value>,
    lookup: &dyn Lookup,
) -> Value {
    let context = object(field(record, FULFILLMENT_ORDER_EVENT));
    let this = json!({"id": text(record, "id"), "status": lower(field(record, "status"))});
    match topic {
        "fulfillment_orders/placed_on_hold" => {
            let holds: Vec<Value> = records(field(record, "fulfillmentHolds"), lookup)
                .iter()
                .map(hold)
                .collect();
            json!({
                "fulfillment_order": {"id": text(record, "id"), "status": lower(field(record, "status")), "fulfillment_holds": holds},
                "remaining_fulfillment_order": fulfillment_order_ref(field(&context, "remaining"), lookup),
                "held_fulfillment_order_line_items": line_quantities(field(&context, "lineItems")),
                "created_fulfillment_hold": resolve(field(&context, "hold"), lookup).as_ref().map_or(Value::Null, hold),
            })
        }
        "fulfillment_orders/cancelled" => json!({
            "fulfillment_order": this,
            "replacement_fulfillment_order": fulfillment_order_ref(field(&context, "replacement"), lookup),
        }),
        "fulfillment_orders/moved" => {
            let original = resolve(field(&context, "original"), lookup);
            let assigned = |record: &Map<String, Value>| {
                field(record, "assignedLocation")
                    .get("location")
                    .cloned()
                    .unwrap_or(Value::Null)
            };
            json!({
                "original_fulfillment_order": original.as_ref().map_or(Value::Null, |original| json!({
                    "id": text(original, "id"),
                    "status": lower(field(original, "status")),
                    "assigned_location_id": ref_gid(&assigned(original)),
                })),
                "moved_fulfillment_order": {
                    "id": text(record, "id"),
                    "status": lower(field(record, "status")),
                    "assigned_location_id": ref_gid(&assigned(record)),
                },
                "destination_location_id": ref_gid(&assigned(record)),
                "fulfillment_order_line_items_requested": line_quantities(field(&context, "lineItems")),
                "source_location": {"id": ref_gid(field(&context, "source"))},
            })
        }
        "fulfillment_orders/rescheduled" => json!({
            "fulfillment_order": {"id": text(record, "id"), "status": lower(field(record, "status")), "fulfill_at": time(field(record, "fulfillAt"))},
        }),
        "fulfillment_orders/split" => json!({
            "split_line_items": line_quantities(field(&context, "lineItems")),
            "fulfillment_order": fulfillment_order_ref(field(&context, "original"), lookup),
            "remaining_fulfillment_order": this,
            "replacement_fulfillment_order": fulfillment_order_ref(field(&context, "replacement"), lookup),
        }),
        "fulfillment_orders/fulfillment_request_submitted" => json!({
            "original_fulfillment_order": requested(field(&context, "original"), lookup),
            "submitted_fulfillment_order": requested(field(&context, "submitted"), lookup),
            "fulfillment_order_merchant_request": merchant_request(field(&context, "request"), lookup),
        }),
        "fulfillment_orders/cancellation_request_submitted" => json!({
            "fulfillment_order": requested(field(record, "id"), lookup),
            "fulfillment_order_merchant_request": merchant_request(field(&context, "request"), lookup),
        }),
        "fulfillment_orders/fulfillment_request_accepted"
        | "fulfillment_orders/fulfillment_request_rejected"
        | "fulfillment_orders/cancellation_request_rejected" => json!({
            "fulfillment_order": requested(field(record, "id"), lookup),
            "message": text(&context, "message"),
        }),
        "fulfillment_orders/cancellation_request_accepted"
        | "fulfillment_orders/fulfillment_service_failed_to_complete" => json!({
            "fulfillment_order": this,
            "message": text(&context, "message"),
        }),
        // `hold_released`, `scheduled_fulfillment_order_ready`.
        _ => json!({"fulfillment_order": this}),
    }
}

/// A fulfillment order with its request status, as the request topics write it: REST's
/// `cancellation_request` for `CANCELLATION_REQUESTED` (the example of
/// `cancellation_request_submitted`), the others in lowercase.
fn requested(reference: &Value, lookup: &dyn Lookup) -> Value {
    let Some(record) = resolve(reference, lookup) else {
        return Value::Null;
    };
    let request = match field(&record, "requestStatus").as_str() {
        Some("CANCELLATION_REQUESTED") => json!("cancellation_request"),
        _ => lower(field(&record, "requestStatus")),
    };
    json!({"id": text(&record, "id"), "status": lower(field(&record, "status")), "request_status": request})
}

/// A `FulfillmentOrderMerchantRequest`: its GID and message.
fn merchant_request(reference: &Value, lookup: &dyn Lookup) -> Value {
    match resolve(reference, lookup) {
        Some(record) => json!({"id": text(&record, "id"), "message": text(&record, "message")}),
        None => Value::Null,
    }
}

/// `returns/…`: a `Return`, in the shape of the example of its topic.
pub(super) fn return_event(topic: &str, record: &Map<String, Value>) -> Value {
    let head = json!({"id": id(record), "admin_graphql_api_id": text(record, "id")});
    let order = json!({"id": ref_id(field(record, "order")), "admin_graphql_api_id": ref_gid(field(record, "order"))});
    match topic {
        "returns/request" | "returns/approve" => merge([
            head,
            json!({
                "status": lower(field(record, "status")),
                "order": order,
                "total_return_line_items": items(field(record, "returnLineItems")).len(),
                "name": text(record, "name"),
                // The example's lists are empty; lsf does not know the shape of their items.
                "return_line_items": [],
                "return_shipping_fees": [],
                "exchange_line_items": [],
                "total_exchange_line_items": items(field(record, "exchangeLineItems")).len(),
                "requested_at": time(first(record, &["requestedAt", "createdAt"])),
            }),
        ]),
        "returns/decline" => merge([
            head,
            json!({
                "status": lower(field(record, "status")),
                "order": order,
                "decline": match field(record, "decline") {
                    Value::Object(decline) => json!({"reason": lower(field(decline, "reason")), "note": text(decline, "note")}),
                    _ => Value::Null,
                },
            }),
        ]),
        // `close`, `cancel`, `reopen`.
        _ => merge([
            head,
            json!({"order_id": ref_id(field(record, "order")), "status": lower(field(record, "status"))}),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_add_up_in_hundredths() {
        assert_eq!(cents(&json!("19.99")), Some(1999));
        assert_eq!(cents(&json!("19.9")), Some(1990));
        assert_eq!(cents(&json!("19")), Some(1900));
        assert_eq!(cents(&json!("0.125")), Some(13));
        assert_eq!(cents(&json!("-5.5")), Some(-550));
        assert_eq!(cents(&json!(12.5)), Some(1250));
        assert_eq!(cents(&json!("x")), None);
        assert_eq!(decimal(-150), "-1.50");
        assert_eq!(decimal(-5), "-0.05");
        assert_eq!(decimal(1999), "19.99");
        let total = sum_set(
            [Some((1999, json!("EUR"))), None, Some((1, json!("EUR")))].into_iter(),
            &Value::Null,
        );
        assert_eq!(
            total["shop_money"],
            json!({"amount": "20.00", "currency_code": "EUR"})
        );
    }

    #[test]
    fn the_token_of_an_order_is_that_of_its_status_page() {
        let mut order = Map::new();
        order.insert("id".into(), json!("gid://shopify/Order/1"));
        order.insert(
            "statusPageUrl".into(),
            json!("https://shop.example/1/orders/abc123/authenticate?key=x"),
        );
        assert_eq!(token(&order), json!("abc123"));
        order.remove("statusPageUrl");
        let derived = token(&order);
        assert_eq!(derived.as_str().map(str::len), Some(32));
        assert_eq!(token(&order), derived);
    }
}
