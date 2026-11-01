//! The rest of returns: exchanges, fees, reverse fulfillment orders and their deliveries and
//! dispositions, lines removed from a return, `returnRefund`, and what a return would refund
//! (`returnCalculate`, `Return.suggestedFinancialOutcome`, `Return.suggestedRefund`).
//!
//! As shopify.dev's reference of 2026-10 describes them (`mutations/<name>`, `objects/
//! ReverseFulfillmentOrder`, `objects/ExchangeLineItem`, `queries/returnCalculate`, and the
//! returns apps tutorials, `apps/build/orders-fulfillment/returns-apps`):
//!
//! - **Exchanges**: a return's `exchangeLineItems` (a variant, a quantity, an applied discount)
//!   are `ExchangeLineItem`s, processable once the return is open. `returnProcess` processes
//!   them: each becomes a line item of the order (on an open fulfillment order, its units
//!   committed), and the return line items processed with them are taken back by a refund, so
//!   that the order owes the difference: nothing for an even exchange, the balance (an
//!   `InvoiceReturnOutcome`) when the exchange costs more, a refund when it costs less. Their tax
//!   is the order's rate (the store data's, else the order's own).
//! - **Fees**: a return line item's `restockingFee` (a percentage of its value) and the return's
//!   `returnShippingFee` are kept on the return and taken off what it suggests to refund.
//! - **Reverse fulfillment orders**: an open return has one, with a line item per return line
//!   item; it closes and is canceled with its return. `reverseDeliveryCreateWithShipping` and
//!   `reverseDeliveryShippingUpdate` give a delivery of some of its units its tracking and label
//!   (`reverse_deliveries/attach_deliverable`; the carrier is read from the tracking URL), and
//!   notify the customer by default (`notifyCustomer: true`): the label goes to the shop's
//!   outbox. `reverseFulfillmentOrderDispose` disposes units (`RESTOCKED` puts them back in stock
//!   at its location; `NOT_RESTOCKED`, `MISSING`, `PROCESSING_REQUIRED` do not):
//!   `reverse_fulfillment_orders/dispose`, with the dispositions just made. A disposition of
//!   `returnProcess` is recorded on its line item too.
//! - **`removeFromReturn`** and **`returnLineItemRemoveFromReturn`** take unprocessed units out of
//!   a return (a line left with none goes): `returns/update`, with the removals.
//! - **`returnRefund`** refunds units of an open or closed return (and shipping), through the
//!   order's payments: a refund of the return (`refunds/create`), the units' refunded quantity.
//! - **What a return would refund**: the value of the units (after the order's discounts, with
//!   their share of its tax) and the shipping asked, less the fees and what the exchanged items
//!   cost; `returnCalculate` the same for lines that are not a return yet.
//!
//! Messages: lsf's wording, with the `ReturnErrorCode` the payloads have; shopify.dev shows
//! none.

use serde_json::{Value, json};

use super::LineDraft;
use super::model::{self, array, field_cents, int, text};
use super::refunds::{self, RefundLine, RefundSpec};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::outbox::{self, Entry};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("removeFromReturn", |m, ctx| {
            remove_from_return(m, ctx, true)
        })
        .mutation("returnLineItemRemoveFromReturn", |m, ctx| {
            remove_from_return(m, ctx, false)
        })
        .mutation("returnRefund", return_refund)
        .mutation("reverseDeliveryCreateWithShipping", delivery_create)
        .mutation("reverseDeliveryShippingUpdate", delivery_update)
        .mutation("reverseFulfillmentOrderDispose", dispose)
        .query("returnCalculate", calculate)
        .field("Return", "suggestedFinancialOutcome", suggested_outcome)
        .field("Return", "suggestedRefund", suggested_refund);
}

fn error(field: &[&str], message: &str, code: &str) -> UserError {
    UserError::new(field, message).code(code)
}

/// The field of a reverse fulfillment order that names its return.
const RETURN: &str = "lsfReturn";

/// The field of a reverse fulfillment order line item that names its return line item.
const RETURN_LINE: &str = "lsfReturnLineItem";

/// The dispositions a reverse fulfillment order's last change made: what its
/// `reverse_fulfillment_orders/dispose` lists.
pub const LAST_DISPOSITIONS: &str = "lsfLastDispositions";

// --- Exchanges and fees ---

/// An exchange line item to make: `(variant, quantity, unit price, discount in all)`.
pub struct Exchange {
    variant: String,
    quantity: i64,
    unit: i64,
    discount: i64,
    applied: Value,
}

/// Reads `ExchangeLineItemInput`s (or `CalculateExchangeLineItemInput`s): a variant of the
/// shop, a positive quantity, a discount (a percentage, or an amount off each unit).
pub fn read_exchanges(
    state: &ShopState,
    items: &[Value],
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Vec<Exchange> {
    let mut exchanges = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let index = index.to_string();
        let at = |field: &'static str| [path, &[index.as_str(), field]].concat();
        let variant = item
            .get("variantId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(record) = state
            .get(variant)
            .filter(|_| state.type_of(variant) == Some("ProductVariant"))
        else {
            errors.push(error(
                &at("variantId"),
                "Variant does not exist.",
                "NOT_FOUND",
            ));
            continue;
        };
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if quantity <= 0 {
            errors.push(error(
                &at("quantity"),
                "Exchange line item has an invalid quantity.",
                "INVALID",
            ));
            continue;
        }
        let unit = field_cents(record, "price");
        let applied = item.get("appliedDiscount").cloned().unwrap_or(Value::Null);
        let total = unit.saturating_mul(quantity);
        let discount = match applied.get("value") {
            Some(value) => match (value.get("percentage"), value.get("amount")) {
                (Some(percentage), _) if !percentage.is_null() => {
                    let percentage = percentage.as_f64().unwrap_or(0.0);
                    if !(0.0..=100.0).contains(&percentage) {
                        errors.push(error(
                            &at("appliedDiscount"),
                            "Percentage must be between 0 and 100.",
                            "INVALID",
                        ));
                        continue;
                    }
                    ((total as f64) * percentage / 100.0).round() as i64
                }
                (_, Some(amount)) if !amount.is_null() => model::cents(Some(amount))
                    .max(0)
                    .saturating_mul(quantity)
                    .min(total),
                _ => 0,
            },
            None => 0,
        };
        if !value::money_in_bounds(total) {
            errors.push(error(
                &at("quantity"),
                &value::money_too_large("Total price"),
                "INVALID",
            ));
            continue;
        }
        exchanges.push(Exchange {
            variant: variant.to_string(),
            quantity,
            unit,
            discount,
            applied,
        });
    }
    exchanges
}

/// Makes the `ExchangeLineItem`s of a return, processable when it is open. Returns their GIDs.
pub fn insert_exchanges(state: &mut ShopState, exchanges: &[Exchange], open: bool) -> Vec<String> {
    let mut ids = Vec::new();
    for exchange in exchanges {
        let variant = state.get(&exchange.variant).cloned().unwrap_or_default();
        let product = text(&variant, "product").and_then(|product| state.get(product));
        let product_title = product
            .and_then(|product| text(product, "title"))
            .map(str::to_string);
        let product_id = text(&variant, "product").map(str::to_string);
        let id = state.new_gid("ExchangeLineItem");
        state.insert(value::object(json!({
            "__typename": "ExchangeLineItem",
            "id": id,
            "variantId": exchange.variant,
            "productId": product_id,
            "title": product_title,
            "variantTitle": text(&variant, "title").filter(|title| *title != "Default Title"),
            "variantSku": variant.get("sku"),
            "quantity": exchange.quantity,
            "processedQuantity": 0,
            "unprocessedQuantity": exchange.quantity,
            "processableQuantity": if open { exchange.quantity } else { 0 },
            "lineItem": null,
            "lineItems": [],
            "lsfUnitPrice": exchange.unit,
            "lsfDiscount": exchange.discount,
            "lsfAppliedDiscount": exchange.applied,
        })));
        ids.push(id);
    }
    ids
}

/// A restocking fee of a `RestockingFeeInput` (`{percentage}`) on `subtotal`: `RestockingFee`
/// inline, with an id of the fee's.
pub fn restocking_fee(state: &mut ShopState, input: &Value, subtotal: i64) -> Option<Value> {
    let percentage = input.get("percentage")?.as_f64()?;
    let currency = model::currency(state);
    let id = state.new_gid("RestockingFee");
    Some(json!({
        "id": id,
        "percentage": percentage,
        "amountSet": value::money_bag(((subtotal as f64) * percentage / 100.0).round() as i64, &currency),
    }))
}

/// A return shipping fee of a `ReturnShippingFeeInput` (`{amount}`): `ReturnShippingFee` inline.
pub fn shipping_fee(state: &mut ShopState, input: &Value) -> Option<Value> {
    let amount = model::cents(input.get("amount"));
    let currency = model::currency(state);
    let id = state.new_gid("ReturnShippingFee");
    Some(json!({
        "id": id,
        "amountSet": value::money_bag(amount.max(0), &currency),
        "finalizedTaxAmountSet": null,
    }))
}

/// The order's tax rate for exchanged items, in basis points: its edits' rate, none when the
/// order's prices include their taxes or it is tax exempt.
fn tax_rate(state: &ShopState, order: &Record) -> i64 {
    if order.get("taxesIncluded").and_then(Value::as_bool) == Some(true)
        || order.get("taxExempt").and_then(Value::as_bool) == Some(true)
    {
        return 0;
    }
    super::edits::tax_rate_bps(state, order)
}

/// Processes units of a return's exchange line items: each becomes a line item of the order,
/// at its price after its discount, on an open fulfillment order, its units committed; the
/// order's tax takes theirs.
pub fn process_exchanges(state: &mut ShopState, order: &str, items: &[(String, i64)]) {
    let currency = model::currency(state);
    let now = state.timestamp();
    let Some(order_record) = state.get(order).cloned() else {
        return;
    };
    let rate = tax_rate(state, &order_record);
    let captured = matches!(
        text(&order_record, "displayFinancialStatus"),
        Some("PAID" | "PARTIALLY_PAID" | "PARTIALLY_REFUNDED" | "REFUNDED")
    );
    let mut added_tax = 0;
    for (exchange, quantity) in items {
        let Some(record) = state.get(exchange).cloned() else {
            continue;
        };
        let variant = text(&record, "variantId").map(str::to_string);
        let variant_record = variant
            .as_deref()
            .and_then(|variant| state.get(variant))
            .cloned();
        let ordered = int(&record, "quantity").max(1);
        let discount = model::share(
            record
                .get("lsfDiscount")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            *quantity,
            ordered,
        );
        let inventory_item = variant_record
            .as_ref()
            .and_then(|variant| text(variant, "inventoryItem"))
            .map(str::to_string);
        let item = inventory_item.as_deref().and_then(|item| state.get(item));
        let taxable = variant_record
            .as_ref()
            .and_then(|variant| variant.get("taxable"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let draft = LineDraft {
            product: text(&record, "productId").map(str::to_string),
            variant: variant.clone(),
            inventory_item: inventory_item.clone(),
            title: text(&record, "title").unwrap_or_default().to_string(),
            variant_title: text(&record, "variantTitle").map(str::to_string),
            sku: text(&record, "variantSku").map(str::to_string),
            vendor: None,
            quantity: (*quantity).max(0) as u32,
            unit_price: record
                .get("lsfUnitPrice")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            discount,
            requires_shipping: item
                .and_then(|item| item.get("requiresShipping"))
                .and_then(Value::as_bool)
                .unwrap_or(true),
            taxable,
            gift_card: false,
            image: variant_record
                .as_ref()
                .and_then(|variant| variant.get("image"))
                .filter(|image| !image.is_null())
                .cloned(),
            weight: item
                .and_then(|item| item.get("measurement"))
                .and_then(|measurement| measurement.get("weight"))
                .cloned(),
            grams: 0,
            attributes: Vec::new(),
        };
        let line = state.new_gid("LineItem");
        let mut line_record =
            super::line_item_record(&line, &draft, &currency, draft.quantity, captured, &now);
        if taxable {
            let tax = model::share(
                draft.unit_price * i64::from(draft.quantity) - discount,
                rate,
                10_000,
            );
            added_tax += tax;
            line_record.insert(
                "taxLines".into(),
                json!([{"title": "Tax", "rate": rate as f64 / 10_000.0, "priceSet": value::money_bag(tax, &currency)}]),
            );
        }
        state.insert(line_record);
        model::push_key(state, order, "lineItems", &line);
        let location = model::assigned_location(
            state,
            gids(order_record.get("fulfillmentOrders"))
                .first()
                .map_or("", String::as_str),
        );
        super::edits::add_to_fulfillment(state, order, &line, *quantity, location.as_deref());
        model::commit_line(state, &line, location.as_deref(), *quantity);
        if let Some(exchange_record) = state.get_mut(exchange) {
            let processed = int(exchange_record, "processedQuantity") + quantity;
            let unprocessed = int(exchange_record, "unprocessedQuantity") - quantity;
            exchange_record.insert("processedQuantity".into(), json!(processed));
            exchange_record.insert("unprocessedQuantity".into(), json!(unprocessed));
            exchange_record.insert("processableQuantity".into(), json!(unprocessed));
            exchange_record.insert("lineItem".into(), json!(line));
            if let Some(Value::Array(lines)) = exchange_record.get_mut("lineItems") {
                lines.push(json!(line));
            }
        }
    }
    if added_tax > 0 {
        let tax = field_cents(&order_record, "totalTaxSet") + added_tax;
        if let Some(record) = state.get_mut(order) {
            record.insert("totalTaxSet".into(), value::money_bag(tax, &currency));
            record.insert(
                "taxLines".into(),
                json!([{"title": "Tax", "priceSet": value::money_bag(tax, &currency)}]),
            );
        }
    }
}

// --- Reverse fulfillment orders ---

/// Opens the reverse fulfillment order of a return that has none: a line item per return line
/// item. Returns its GID.
pub fn open_reverse_fulfillment(state: &mut ShopState, return_id: &str) -> Option<String> {
    let record = state.get(return_id)?.clone();
    if let Some(existing) = gids(record.get("reverseFulfillmentOrders")).first() {
        return Some(existing.clone());
    }
    let id = state.new_gid("ReverseFulfillmentOrder");
    let mut lines = Vec::new();
    for return_line in gids(record.get("returnLineItems")) {
        let Some(item) = state.get(&return_line).cloned() else {
            continue;
        };
        let line = state.new_gid("ReverseFulfillmentOrderLineItem");
        state.insert(value::object(json!({
            "__typename": "ReverseFulfillmentOrderLineItem",
            "id": line,
            "fulfillmentLineItem": item.get("fulfillmentLineItem"),
            "totalQuantity": int(&item, "quantity"),
            "dispositions": [],
            RETURN_LINE: return_line,
        })));
        lines.push(line);
    }
    state.insert(value::object(json!({
        "__typename": "ReverseFulfillmentOrder",
        "id": id,
        "order": record.get("order"),
        "status": "OPEN",
        "lineItems": lines,
        "reverseDeliveries": [],
        "thirdPartyConfirmation": null,
        RETURN: return_id,
    })));
    model::push_key(state, return_id, "reverseFulfillmentOrders", &id);
    Some(id)
}

/// The status of a return's reverse fulfillment orders follows the return's: `CLOSED` when it
/// closes, `CANCELED` when it is canceled, `OPEN` when it opens again.
pub fn follow_return(state: &mut ShopState, return_id: &str) {
    let Some(record) = state.get(return_id).cloned() else {
        return;
    };
    let status = match text(&record, "status") {
        Some("CLOSED") => "CLOSED",
        Some("CANCELED" | "DECLINED") => "CANCELED",
        _ => "OPEN",
    };
    for reverse in gids(record.get("reverseFulfillmentOrders")) {
        if let Some(rfo) = state.get_mut(&reverse) {
            rfo.insert("status".into(), json!(status));
        }
    }
}

/// The reverse fulfillment order line item of a return line item.
fn reverse_line_of(state: &ShopState, return_id: &str, return_line: &str) -> Option<String> {
    let record = state.get(return_id)?;
    gids(record.get("reverseFulfillmentOrders"))
        .iter()
        .filter_map(|rfo| state.get(rfo))
        .flat_map(|rfo| gids(rfo.get("lineItems")))
        .find(|line| state.get(line).and_then(|item| text(item, RETURN_LINE)) == Some(return_line))
}

/// What is disposed of a reverse fulfillment order line item.
fn disposed(state: &ShopState, line: &Record) -> i64 {
    gids(line.get("dispositions"))
        .iter()
        .filter_map(|id| state.get(id))
        .map(|disposition| int(disposition, "quantity"))
        .sum()
}

/// Records a disposition on a reverse fulfillment order line item. Returns its GID.
fn add_disposition(
    state: &mut ShopState,
    line: &str,
    quantity: i64,
    kind: &str,
    location: Option<&str>,
) -> String {
    let id = state.new_gid("ReverseFulfillmentOrderDisposition");
    let now = state.timestamp();
    state.insert(value::object(json!({
        "__typename": "ReverseFulfillmentOrderDisposition",
        "id": id,
        "quantity": quantity,
        "type": kind,
        "location": location,
        "createdAt": now,
        "lsfLineItem": line,
    })));
    model::push_key(state, line, "dispositions", &id);
    id
}

/// Records what `returnProcess` disposed of a return line item on its reverse fulfillment order
/// line item (`RESTOCKED` at the location given, else `NOT_RESTOCKED`). The stock is
/// `returnProcess`'s to move.
pub fn record_processed(
    state: &mut ShopState,
    return_id: &str,
    return_line: &str,
    quantity: i64,
    restock: Option<Option<String>>,
) {
    let Some(line) = reverse_line_of(state, return_id, return_line) else {
        return;
    };
    let (kind, location) = match restock {
        Some(location) => ("RESTOCKED", location),
        None => ("NOT_RESTOCKED", None),
    };
    add_disposition(state, &line, quantity, kind, location.as_deref());
}

/// `reverseFulfillmentOrderDispose`.
fn dispose(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let failed = |error: UserError| {
        Ok(payload(
            json!({"reverseFulfillmentOrderLineItems": null}),
            &[error],
        ))
    };
    let inputs = ctx
        .arg("dispositionInputs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if inputs.is_empty() {
        return failed(error(
            &["dispositionInputs"],
            "Disposition inputs can't be blank.",
            "BLANK",
        ));
    }
    let state = mutation.state();
    let mut planned: Vec<(String, i64, String, Option<String>)> = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let index = index.to_string();
        let at = |field: &'static str| ["dispositionInputs", index.as_str(), field];
        let line = input
            .get("reverseFulfillmentOrderLineItemId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(record) = state
            .get(line)
            .filter(|_| state.type_of(line) == Some("ReverseFulfillmentOrderLineItem"))
        else {
            return failed(error(
                &at("reverseFulfillmentOrderLineItemId"),
                "Reverse fulfillment order line item does not exist.",
                "NOT_FOUND",
            ));
        };
        let quantity = input.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        let already: i64 = planned
            .iter()
            .filter(|(known, ..)| known == line)
            .map(|(_, quantity, ..)| quantity)
            .sum();
        if quantity <= 0
            || quantity + already + disposed(state, record) > int(record, "totalQuantity")
        {
            return failed(error(
                &at("quantity"),
                "Quantity is more than what is left to dispose of the line item.",
                "INVALID",
            ));
        }
        let kind = input
            .get("dispositionType")
            .and_then(Value::as_str)
            .unwrap_or("NOT_RESTOCKED")
            .to_string();
        let location = input
            .get("locationId")
            .and_then(Value::as_str)
            .map(str::to_string);
        if kind == "RESTOCKED" {
            match &location {
                Some(location) if state.type_of(location) == Some("Location") => {}
                Some(_) => {
                    return failed(error(
                        &at("locationId"),
                        "Location does not exist.",
                        "NOT_FOUND",
                    ));
                }
                None => {
                    return failed(error(
                        &at("locationId"),
                        "A location is required to restock.",
                        "BLANK",
                    ));
                }
            }
        }
        planned.push((line.to_string(), quantity, kind, location));
    }
    let state = mutation.state_mut();
    let mut by_order: Vec<(String, Vec<String>)> = Vec::new();
    let mut lines = Vec::new();
    for (line, quantity, kind, location) in planned {
        let disposition = add_disposition(state, &line, quantity, &kind, location.as_deref());
        // The order's line item, through the fulfillment line item.
        let line_item = state
            .get(&line)
            .and_then(|record| text(record, "fulfillmentLineItem"))
            .and_then(|fulfillment| state.get(fulfillment))
            .and_then(|fulfillment| text(fulfillment, "lineItem"))
            .map(str::to_string);
        if kind == "RESTOCKED"
            && let Some(line_item) = line_item
        {
            model::restock_line(state, &line_item, location.as_deref(), quantity, true);
        }
        let reverse = state
            .keys("ReverseFulfillmentOrder")
            .find(|rfo| {
                state
                    .get(rfo)
                    .is_some_and(|record| gids(record.get("lineItems")).contains(&line))
            })
            .map(str::to_string);
        if let Some(reverse) = reverse {
            match by_order.iter_mut().find(|(known, _)| *known == reverse) {
                Some((_, made)) => made.push(disposition),
                None => by_order.push((reverse, vec![disposition])),
            }
        }
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    for (reverse, made) in by_order {
        if let Some(record) = state.get_mut(&reverse) {
            record.insert(LAST_DISPOSITIONS.into(), json!(made));
        }
        state.emit("reverse_fulfillment_orders/dispose", &reverse);
    }
    Ok(payload(
        json!({"reverseFulfillmentOrderLineItems": lines}),
        &[],
    ))
}

// --- Reverse deliveries ---

/// The carrier of a tracking URL, as Shopify names it for the carriers it reads.
fn carrier_of(url: Option<&str>) -> Option<&'static str> {
    let host = url?
        .split("://")
        .nth(1)?
        .split('/')
        .next()?
        .to_ascii_lowercase();
    [
        ("usps", "USPS"),
        ("ups.", "UPS"),
        ("fedex", "FedEx"),
        ("dhl", "DHL Express"),
        ("canadapost", "Canada Post"),
        ("royalmail", "Royal Mail"),
        ("laposte", "La Poste"),
    ]
    .into_iter()
    .find(|(key, _)| host.contains(key))
    .map(|(_, name)| name)
}

/// The `ReverseDeliveryShippingDeliverable` of a tracking and a label input, over `old`.
fn deliverable(tracking: Option<&Value>, label: Option<&Value>, old: &Value, now: &str) -> Value {
    let mut tracking_value = old.get("tracking").cloned().unwrap_or(Value::Null);
    if let Some(tracking) = tracking.filter(|value| !value.is_null()) {
        let url = tracking.get("url").and_then(Value::as_str);
        tracking_value = json!({
            "number": tracking.get("number"),
            "url": url,
            "carrierName": carrier_of(url),
        });
    }
    let mut label_value = old.get("label").cloned().unwrap_or(Value::Null);
    if let Some(label) = label.filter(|value| !value.is_null()) {
        let created = label_value
            .get("createdAt")
            .cloned()
            .unwrap_or_else(|| json!(now));
        label_value = json!({
            "publicFileUrl": label.get("fileUrl"),
            "createdAt": created,
            "updatedAt": now,
        });
    }
    json!({
        "__typename": "ReverseDeliveryShippingDeliverable",
        "tracking": tracking_value,
        "label": label_value,
    })
}

/// Records the return label's email in the outbox, for `notifyCustomer`.
fn notify_label(state: &mut ShopState, delivery: &str) {
    let Some(record) = state.get(delivery).cloned() else {
        return;
    };
    let order = text(&record, "reverseFulfillmentOrder")
        .and_then(|rfo| state.get(rfo))
        .and_then(|rfo| text(rfo, "order"))
        .and_then(|order| state.get(order))
        .cloned();
    let Some(order) = order else {
        return;
    };
    let Some(email) = text(&order, "email").map(str::to_string) else {
        return;
    };
    let name = text(&order, "name").unwrap_or_default().to_string();
    let deliverable = record.get("deliverable").cloned().unwrap_or(Value::Null);
    let label = deliverable
        .pointer("/label/publicFileUrl")
        .and_then(Value::as_str)
        .map(str::to_string);
    let tracking = deliverable
        .pointer("/tracking/number")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut body = format!("Your return for order {name} is on its way back.");
    if let Some(label) = label {
        body.push_str(&format!("\nReturn label: {label}"));
    }
    if let Some(tracking) = tracking {
        body.push_str(&format!("\nTracking number: {tracking}"));
    }
    outbox::record(
        state,
        Entry::email("return_label", &email)
            .subject(format!("Return label for order {name}"))
            .body(body)
            .about(delivery),
    );
}

/// `reverseDeliveryCreateWithShipping`.
fn delivery_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| Ok(payload(json!({"reverseDelivery": null}), &[error]));
    let reverse = ctx
        .arg_str("reverseFulfillmentOrderId")
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(record) = state
        .get(&reverse)
        .filter(|_| state.type_of(&reverse) == Some("ReverseFulfillmentOrder"))
        .cloned()
    else {
        return failed(error(
            &["reverseFulfillmentOrderId"],
            "Reverse fulfillment order does not exist.",
            "NOT_FOUND",
        ));
    };
    if text(&record, "status") != Some("OPEN") {
        return failed(error(
            &["reverseFulfillmentOrderId"],
            "Reverse fulfillment order is not open.",
            "INVALID_STATE",
        ));
    }
    let items = ctx
        .arg("reverseDeliveryLineItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return failed(error(
            &["reverseDeliveryLineItems"],
            "Reverse delivery line items can't be blank.",
            "BLANK",
        ));
    }
    let own = gids(record.get("lineItems"));
    let mut planned: Vec<(String, i64)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let index = index.to_string();
        let line = item
            .get("reverseFulfillmentOrderLineItemId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !own.iter().any(|known| known == line) {
            return failed(error(
                &[
                    "reverseDeliveryLineItems",
                    &index,
                    "reverseFulfillmentOrderLineItemId",
                ],
                "Reverse fulfillment order line item does not exist.",
                "NOT_FOUND",
            ));
        }
        // What other deliveries carry of the line already.
        let carried: i64 = gids(record.get("reverseDeliveries"))
            .iter()
            .filter_map(|delivery| state.get(delivery))
            .flat_map(|delivery| gids(delivery.get("reverseDeliveryLineItems")))
            .filter_map(|id| state.get(&id))
            .filter(|item| text(item, "reverseFulfillmentOrderLineItem") == Some(line))
            .map(|item| int(item, "quantity"))
            .sum();
        let total = state.get(line).map_or(0, |item| int(item, "totalQuantity"));
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if quantity <= 0 || quantity + carried > total {
            return failed(error(
                &["reverseDeliveryLineItems", &index, "quantity"],
                "Quantity is more than what is left to deliver of the line item.",
                "INVALID",
            ));
        }
        planned.push((line.to_string(), quantity));
    }
    let notify = ctx.arg_bool("notifyCustomer").unwrap_or(true);
    let (tracking, label) = (
        ctx.arg("trackingInput").cloned(),
        ctx.arg("labelInput").cloned(),
    );
    let state = mutation.state_mut();
    let now = state.timestamp();
    let id = state.new_gid("ReverseDelivery");
    let mut lines = Vec::new();
    for (line, quantity) in planned {
        let item = state.new_gid("ReverseDeliveryLineItem");
        state.insert(value::object(json!({
            "__typename": "ReverseDeliveryLineItem",
            "id": item,
            "quantity": quantity,
            "reverseFulfillmentOrderLineItem": line,
            "dispositions": [],
        })));
        lines.push(item);
    }
    let has_deliverable = tracking.as_ref().is_some_and(|value| !value.is_null())
        || label.as_ref().is_some_and(|value| !value.is_null());
    state.insert(value::object(json!({
        "__typename": "ReverseDelivery",
        "id": id,
        "reverseFulfillmentOrder": reverse,
        "reverseDeliveryLineItems": lines,
        "deliverable": if has_deliverable {
            deliverable(tracking.as_ref(), label.as_ref(), &Value::Null, &now)
        } else {
            Value::Null
        },
    })));
    model::push_key(state, &reverse, "reverseDeliveries", &id);
    if has_deliverable {
        state.emit("reverse_deliveries/attach_deliverable", &id);
        if notify {
            notify_label(state, &id);
        }
    }
    Ok(payload(json!({"reverseDelivery": id}), &[]))
}

/// `reverseDeliveryShippingUpdate`.
fn delivery_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| Ok(payload(json!({"reverseDelivery": null}), &[error]));
    let id = ctx
        .arg_str("reverseDeliveryId")
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(record) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("ReverseDelivery"))
        .cloned()
    else {
        return failed(error(
            &["reverseDeliveryId"],
            "Reverse delivery does not exist.",
            "NOT_FOUND",
        ));
    };
    let (tracking, label) = (
        ctx.arg("trackingInput").cloned(),
        ctx.arg("labelInput").cloned(),
    );
    if tracking.as_ref().is_none_or(Value::is_null) && label.as_ref().is_none_or(Value::is_null) {
        return failed(error(
            &["trackingInput"],
            "A tracking or a label is required.",
            "BLANK",
        ));
    }
    let notify = ctx.arg_bool("notifyCustomer").unwrap_or(true);
    let state = mutation.state_mut();
    let now = state.timestamp();
    let old = record.get("deliverable").cloned().unwrap_or(Value::Null);
    if let Some(record) = state.get_mut(&id) {
        record.insert(
            "deliverable".into(),
            deliverable(tracking.as_ref(), label.as_ref(), &old, &now),
        );
    }
    state.emit("reverse_deliveries/attach_deliverable", &id);
    if notify {
        notify_label(state, &id);
    }
    Ok(payload(json!({"reverseDelivery": id}), &[]))
}

// --- Removals ---

/// `removeFromReturn` (`exchanges`: its exchange line items too) and
/// `returnLineItemRemoveFromReturn`.
fn remove_from_return(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    exchanges: bool,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| Ok(payload(json!({"return": null}), &[error]));
    let id = ctx.arg_str("returnId").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(record) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("Return"))
        .cloned()
    else {
        return failed(error(&["returnId"], "Return does not exist.", "NOT_FOUND"));
    };
    if !matches!(text(&record, "status"), Some("OPEN" | "REQUESTED")) {
        return failed(error(
            &["returnId"],
            "Return is not editable. Only returns with status OPEN or REQUESTED can be changed.",
            "INVALID_STATE",
        ));
    }
    let mut removals: Vec<(String, i64, &'static str)> = Vec::new();
    let groups: &[(&str, &str, &str, &'static str)] = if exchanges {
        &[
            (
                "returnLineItems",
                "returnLineItemId",
                "returnLineItems",
                "return",
            ),
            (
                "exchangeLineItems",
                "exchangeLineItemId",
                "exchangeLineItems",
                "exchange",
            ),
        ]
    } else {
        &[(
            "returnLineItems",
            "returnLineItemId",
            "returnLineItems",
            "return",
        )]
    };
    for (argument, key, field, kind) in groups {
        let own = gids(record.get(*field));
        for (index, item) in ctx
            .arg(argument)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let index = index.to_string();
            let line = item.get(*key).and_then(Value::as_str).unwrap_or_default();
            if !own.iter().any(|known| known == line) {
                return failed(error(
                    &[argument, &index, key],
                    "Line item does not exist on the return.",
                    "NOT_FOUND",
                ));
            }
            let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
            let unprocessed = state
                .get(line)
                .map_or(0, |item| int(item, "unprocessedQuantity"));
            let already: i64 = removals
                .iter()
                .filter(|(known, ..)| known == line)
                .map(|(_, quantity, _)| quantity)
                .sum();
            if quantity <= 0 || quantity + already > unprocessed {
                return failed(error(
                    &[argument, &index, "quantity"],
                    "Quantity is more than what is unprocessed of the line item.",
                    "INVALID",
                ));
            }
            removals.push((line.to_string(), quantity, kind));
        }
    }
    if removals.is_empty() {
        return failed(error(
            &["returnLineItems"],
            "Return line items can't be blank.",
            "BLANK",
        ));
    }
    let left: i64 = gids(record.get("returnLineItems"))
        .iter()
        .filter_map(|line| state.get(line))
        .map(|line| int(line, "quantity"))
        .sum::<i64>()
        - removals
            .iter()
            .filter(|(_, _, kind)| *kind == "return")
            .map(|(_, quantity, _)| quantity)
            .sum::<i64>();
    if left <= 0 {
        return failed(error(
            &["returnLineItems"],
            "A return must keep at least one return line item: cancel it instead.",
            "INVALID",
        ));
    }
    let state = mutation.state_mut();
    let mut reported = Vec::new();
    for (line, quantity, kind) in &removals {
        let Some(item) = state.get(line).cloned() else {
            continue;
        };
        let remaining = int(&item, "quantity") - quantity;
        if kind == &"return" {
            reported.push(json!({"id": line, "delta": quantity}));
            if let Some(reverse) = reverse_line_of(state, &id, line) {
                if remaining == 0 {
                    remove_reverse_line(state, &id, &reverse);
                } else if let Some(reverse_line) = state.get_mut(&reverse) {
                    reverse_line.insert("totalQuantity".into(), json!(remaining));
                }
            }
        }
        if remaining == 0 {
            let field = if kind == &"return" {
                "returnLineItems"
            } else {
                "exchangeLineItems"
            };
            if let Some(record) = state.get_mut(&id)
                && let Some(Value::Array(items)) = record.get_mut(field)
            {
                items.retain(|known| known.as_str() != Some(line.as_str()));
            }
            state.remove(line);
            continue;
        }
        if let Some(record) = state.get_mut(line) {
            for field in ["quantity", "unprocessedQuantity", "refundableQuantity"] {
                let current = int(record, field);
                record.insert(field.into(), json!((current - quantity).max(0)));
            }
            let processable = int(record, "processableQuantity");
            record.insert(
                "processableQuantity".into(),
                json!((processable - quantity).max(0)),
            );
        }
    }
    let total: i64 = state
        .get(&id)
        .map(|record| gids(record.get("returnLineItems")))
        .unwrap_or_default()
        .iter()
        .filter_map(|line| state.get(line))
        .map(|line| int(line, "quantity"))
        .sum();
    if let Some(record) = state.get_mut(&id) {
        record.insert("totalQuantity".into(), json!(total));
        record.insert(super::returns::UPDATE_REMOVALS.into(), json!(reported));
    }
    if let Some(order) = text(&record, "order") {
        model::refresh_order(state, order);
    }
    state.emit("returns/update", &id);
    Ok(payload(json!({"return": id}), &[]))
}

/// Takes a reverse fulfillment order line item off its order.
fn remove_reverse_line(state: &mut ShopState, return_id: &str, line: &str) {
    let reverses = state
        .get(return_id)
        .map(|record| gids(record.get("reverseFulfillmentOrders")))
        .unwrap_or_default();
    for reverse in reverses {
        if let Some(record) = state.get_mut(&reverse)
            && let Some(Value::Array(items)) = record.get_mut("lineItems")
        {
            items.retain(|known| known.as_str() != Some(line));
        }
    }
    state.remove(line);
}

// --- returnRefund ---

/// `returnRefund`: a refund of units of an open or closed return, and of shipping, through the
/// order's payments.
fn return_refund(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let failed = |errors: &[UserError]| Ok(payload(json!({"refund": null}), errors));
    let input = model::input_of(ctx.arg("returnRefundInput"));
    let id = input
        .get("returnId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(record) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("Return"))
        .cloned()
    else {
        return failed(&[error(
            &["returnRefundInput", "returnId"],
            "Return does not exist.",
            "NOT_FOUND",
        )]);
    };
    if !matches!(text(&record, "status"), Some("OPEN" | "CLOSED")) {
        return failed(&[error(
            &["returnRefundInput", "returnId"],
            "Return is not refundable. Only returns with status OPEN or CLOSED can be refunded.",
            "INVALID_STATE",
        )]);
    }
    let order = text(&record, "order").unwrap_or_default().to_string();
    let order_record = state.get(&order).cloned().unwrap_or_default();
    let own = gids(record.get("returnLineItems"));
    let mut errors = Vec::new();
    let mut lines: Vec<(String, String, i64)> = Vec::new();
    for (index, item) in array(&input, "returnRefundLineItems").iter().enumerate() {
        let index = index.to_string();
        let path = ["returnRefundInput", "returnRefundLineItems", index.as_str()];
        let line = item
            .get("returnLineItemId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !own.iter().any(|known| known == line) {
            errors.push(error(
                &[&path[..], &["returnLineItemId"]].concat(),
                "Return line item does not exist.",
                "NOT_FOUND",
            ));
            continue;
        }
        let Some(item_record) = state.get(line) else {
            continue;
        };
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if quantity <= 0 || quantity > int(item_record, "refundableQuantity") {
            errors.push(error(
                &[&path[..], &["quantity"]].concat(),
                "Return line item has an invalid quantity.",
                "INVALID",
            ));
            continue;
        }
        let line_item = text(item_record, "fulfillmentLineItem")
            .and_then(|fulfillment| state.get(fulfillment))
            .and_then(|fulfillment| text(fulfillment, "lineItem"))
            .map(str::to_string);
        let Some(line_item) = line_item else {
            errors.push(error(
                &[&path[..], &["returnLineItemId"]].concat(),
                "Return line item is not of a fulfilled line item.",
                "INVALID",
            ));
            continue;
        };
        lines.push((line.to_string(), line_item, quantity));
    }
    let payments = model::refundable_payments(state, &order_record);
    let mut moves = Vec::new();
    for (index, transaction) in array(&input, "orderTransactions").iter().enumerate() {
        let index = index.to_string();
        let parent = transaction
            .get("parentId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let amount = model::cents(transaction.get("transactionAmount"));
        let path = ["returnRefundInput", "orderTransactions", index.as_str()];
        let Some((_, left)) = payments.iter().find(|(known, _)| known == parent) else {
            errors.push(error(
                &[&path[..], &["parentId"]].concat(),
                "Parent transaction does not exist.",
                "NOT_FOUND",
            ));
            continue;
        };
        if amount <= 0 || amount > *left {
            errors.push(error(
                &[&path[..], &["transactionAmount"]].concat(),
                "Transaction amount is invalid.",
                "INVALID",
            ));
            continue;
        }
        let gateway = state
            .get(parent)
            .and_then(|record| text(record, "gateway"))
            .map(str::to_string);
        moves.push((amount, gateway, Some(parent.to_string())));
    }
    if lines.is_empty() && errors.is_empty() {
        errors.push(error(
            &["returnRefundInput", "returnRefundLineItems"],
            "Return refund line items can't be blank.",
            "BLANK",
        ));
    }
    let shipping_input = model::input_of(input.get("refundShipping"));
    let remaining_shipping = refunds::refundable_shipping(&order_record);
    let shipping = if shipping_input.get("fullRefund").and_then(Value::as_bool) == Some(true) {
        remaining_shipping
    } else {
        model::cents(shipping_input.get("shippingRefundAmount"))
    };
    if shipping < 0 || shipping > remaining_shipping {
        errors.push(error(
            &["returnRefundInput", "refundShipping"],
            "Shipping must have amount less than or equal to remaining shipping",
            "INVALID",
        ));
    }
    if !errors.is_empty() {
        return failed(&errors);
    }
    if input.get("notifyCustomer").and_then(Value::as_bool) == Some(true) {
        mutation.note("returnRefund: lsf records no refund notice: `notifyCustomer` was ignored");
    }
    let state = mutation.state_mut();
    let refund = refunds::create_refund(
        state,
        &order,
        &RefundSpec {
            lines: lines
                .iter()
                .map(|(_, line_item, quantity)| RefundLine {
                    line: line_item.clone(),
                    quantity: *quantity,
                    restock_type: "NO_RESTOCK".into(),
                    location: None,
                })
                .collect(),
            shipping,
            transactions: moves,
            note: None,
            processed_at: None,
            return_id: Some(id.clone()),
            store_credit: Vec::new(),
            cancellation: false,
        },
    );
    for (line, _, quantity) in &lines {
        if let Some(item) = state.get_mut(line) {
            let refunded = int(item, "refundedQuantity") + quantity;
            let refundable = int(item, "refundableQuantity") - quantity;
            item.insert("refundedQuantity".into(), json!(refunded));
            item.insert("refundableQuantity".into(), json!(refundable.max(0)));
        }
    }
    model::settle_financial_status(state, &order);
    model::order_updated(state, &order);
    Ok(payload(json!({"refund": refund}), &[]))
}

// --- What a return would refund ---

/// What a return (or lines that are not a return yet) would give back: `subtotal` after the
/// order's discounts, its `tax`, the order's discount it bore (`cart`), the fees taken off, and
/// what the exchanged items cost with their tax.
#[derive(Default)]
struct Outcome {
    subtotal: i64,
    before_discounts: i64,
    tax: i64,
    cart: i64,
    fees: i64,
    exchange: i64,
    exchange_tax: i64,
}

impl Outcome {
    /// Adds `quantity` units of a fulfillment line item of `order`, with a restocking fee of
    /// `percentage` when there is one. Returns the line's `(subtotal, tax, before order
    /// discounts, fee)`.
    fn add_line(
        &mut self,
        state: &ShopState,
        order: &Record,
        fulfillment_line: &str,
        quantity: i64,
        percentage: Option<f64>,
    ) -> (i64, i64, i64, i64) {
        let Some(line_item) = state
            .get(fulfillment_line)
            .and_then(|line| text(line, "lineItem"))
        else {
            return (0, 0, 0, 0);
        };
        let (_, subtotal, tax) = refunds::line_amounts(state, order, line_item, quantity);
        let before = state.get(line_item).map_or(0, |item| {
            model::share(
                field_cents(item, "discountedTotalSet"),
                quantity,
                int(item, "quantity").max(1),
            )
        });
        let fee = percentage.map_or(0, |percentage| {
            ((subtotal as f64) * percentage / 100.0).round() as i64
        });
        self.subtotal += subtotal;
        self.before_discounts += before;
        self.tax += tax;
        self.cart += (before - subtotal).max(0);
        self.fees += fee;
        (subtotal, tax, before, fee)
    }

    /// Adds `quantity` units of an exchange at `unit` (cents) less `discount` in all, with the
    /// order's tax on them. Returns `(subtotal, tax)`.
    fn add_exchange(
        &mut self,
        state: &ShopState,
        order: &Record,
        unit: i64,
        quantity: i64,
        discount: i64,
    ) -> (i64, i64) {
        let subtotal = unit * quantity - discount;
        let tax = model::share(subtotal, tax_rate(state, order), 10_000);
        self.exchange += subtotal;
        self.exchange_tax += tax;
        (subtotal, tax)
    }

    /// What the merchant owes the customer: positive a refund, negative what the customer owes.
    fn balance(&self, shipping: i64) -> i64 {
        self.subtotal + self.tax + shipping - self.fees - self.exchange - self.exchange_tax
    }
}

/// The shipping a `RefundShippingInput` asks to refund, at most what is left of the order's.
fn refund_shipping(order: &Record, input: Option<&Value>) -> (i64, i64) {
    let remaining = refunds::refundable_shipping(order);
    let input = model::input_of(input);
    let asked = if input.get("fullRefund").and_then(Value::as_bool) == Some(true) {
        remaining
    } else {
        model::cents(input.get("shippingRefundAmount"))
    };
    (asked.clamp(0, remaining), remaining)
}

/// The suggested transactions of a refund of `amount`: over the order's payments, in order.
fn suggested_transactions(state: &ShopState, order: &Record, amount: i64) -> (Vec<Value>, i64) {
    let currency = model::currency(state);
    let bag = |cents: i64| value::money_bag(cents, &currency);
    let payments = model::refundable_payments(state, order);
    let maximum: i64 = payments.iter().map(|(_, left)| left).sum();
    let mut left = amount.min(maximum);
    let mut suggested = Vec::new();
    for (payment, refundable) in &payments {
        if left <= 0 {
            break;
        }
        let take = left.min(*refundable);
        left -= take;
        let record = state.get(payment);
        suggested.push(json!({
            "kind": "SUGGESTED_REFUND",
            "amountSet": bag(take),
            "maximumRefundableSet": bag(*refundable),
            "gateway": record.and_then(|record| record.get("gateway")),
            "formattedGateway": record.and_then(|record| record.get("formattedGateway")),
            "parentTransaction": payment,
            "accountNumber": null,
            "paymentDetails": null,
        }));
    }
    (suggested, maximum)
}

/// The return line items and exchange line items an outcome is asked for (`[{id, quantity}]`),
/// of the return.
fn asked(ctx: &FieldContext<'_>, argument: &str, own: &[String]) -> Vec<(String, i64)> {
    ctx.arg(argument)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item
                .get("id")
                .or_else(|| item.get("returnLineItemId"))?
                .as_str()?;
            own.iter().any(|known| known == id).then(|| {
                (
                    id.to_string(),
                    item.get("quantity")
                        .and_then(Value::as_i64)
                        .unwrap_or(0)
                        .max(0),
                )
            })
        })
        .collect()
}

/// The outcome of a return's lines and exchanges, as `Return.suggestedFinancialOutcome` and
/// `Return.suggestedRefund` ask for it.
fn return_outcome(
    state: &ShopState,
    record: &Record,
    lines: &[(String, i64)],
    exchanges: &[(String, i64)],
) -> (Outcome, Record) {
    let order = text(record, "order")
        .and_then(|order| state.get(order))
        .cloned()
        .unwrap_or_default();
    let mut outcome = Outcome::default();
    for (line, quantity) in lines {
        let Some(item) = state.get(line) else {
            continue;
        };
        let Some(fulfillment) = text(item, "fulfillmentLineItem") else {
            continue;
        };
        let percentage = item
            .get("restockingFee")
            .and_then(|fee| fee.get("percentage"))
            .and_then(Value::as_f64);
        outcome.add_line(
            state,
            &order,
            fulfillment,
            (*quantity).min(int(item, "quantity")),
            percentage,
        );
    }
    for (exchange, quantity) in exchanges {
        let Some(item) = state.get(exchange) else {
            continue;
        };
        let ordered = int(item, "quantity").max(1);
        let quantity = (*quantity).min(ordered);
        let discount = model::share(
            item.get("lsfDiscount").and_then(Value::as_i64).unwrap_or(0),
            quantity,
            ordered,
        );
        outcome.add_exchange(
            state,
            &order,
            item.get("lsfUnitPrice")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            quantity,
            discount,
        );
    }
    outcome.fees += model::array(record, "returnShippingFees")
        .iter()
        .map(|fee| model::cents(fee.get("amountSet")))
        .sum::<i64>();
    (outcome, order)
}

/// `Return.suggestedFinancialOutcome`.
fn suggested_outcome(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(record) = r.map_of(object).cloned() else {
        return Ok(Output::Null);
    };
    let state = r.state();
    let currency = model::currency(state);
    let bag = |cents: i64| value::money_bag(cents, &currency);
    let lines = asked(ctx, "returnLineItems", &gids(record.get("returnLineItems")));
    let exchanges = asked(
        ctx,
        "exchangeLineItems",
        &gids(record.get("exchangeLineItems")),
    );
    let (outcome, order) = return_outcome(state, &record, &lines, &exchanges);
    let (shipping, remaining_shipping) = refund_shipping(&order, ctx.arg("refundShipping"));
    let balance = outcome.balance(shipping);
    let (transactions, maximum) = suggested_transactions(state, &order, balance.max(0));
    let store_credit = ctx.arg_str("refundMethodAllocation") == Some("STORE_CREDIT");
    let transfer = if balance < 0 {
        json!({"__typename": "InvoiceReturnOutcome", "amount": bag(-balance)})
    } else if store_credit {
        json!({
            "__typename": "RefundReturnOutcome",
            "amount": bag(balance),
            "suggestedTransactions": [],
            "suggestedRefundMethods": [{"__typename": "SuggestedStoreCreditRefund", "amount": bag(balance), "maximumRefundable": bag(balance), "expiresAt": null}],
        })
    } else {
        json!({
            "__typename": "RefundReturnOutcome",
            "amount": bag(balance.min(maximum)),
            "suggestedTransactions": transactions,
            "suggestedRefundMethods": [],
        })
    };
    Ok(Output::Object(Obj::built(
        "SuggestedReturnFinancialOutcome",
        value::object(json!({
            "discountedSubtotal": bag(outcome.subtotal),
            "financialTransfer": transfer,
            "maximumRefundable": bag(maximum),
            "refundDuties": [],
            "shipping": {"amountSet": bag(shipping), "maximumRefundableSet": bag(remaining_shipping), "taxSet": bag(0)},
            "totalAdditionalFees": bag(0),
            "totalCartDiscountAmount": bag(outcome.cart),
            "totalDuties": bag(0),
            "totalTax": bag(outcome.tax),
        })),
    )))
}

/// `Return.suggestedRefund`, the deprecated form of `suggestedFinancialOutcome`.
fn suggested_refund(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(record) = r.map_of(object).cloned() else {
        return Ok(Output::Null);
    };
    let state = r.state();
    let currency = model::currency(state);
    let bag = |cents: i64| value::money_bag(cents, &currency);
    let lines = asked(
        ctx,
        "returnRefundLineItems",
        &gids(record.get("returnLineItems")),
    );
    let (outcome, order) = return_outcome(state, &record, &lines, &[]);
    let (shipping, remaining_shipping) = refund_shipping(&order, ctx.arg("refundShipping"));
    let balance = outcome.balance(shipping).max(0);
    let (transactions, maximum) = suggested_transactions(state, &order, balance);
    Ok(Output::Object(Obj::built(
        "SuggestedReturnRefund",
        value::object(json!({
            "amount": bag(balance.min(maximum)),
            "discountedSubtotal": bag(outcome.subtotal),
            "subtotal": bag(outcome.before_discounts),
            "maximumRefundable": bag(maximum),
            "refundDuties": [],
            "shipping": {"amountSet": bag(shipping), "maximumRefundableSet": bag(remaining_shipping), "taxSet": bag(0)},
            "suggestedTransactions": transactions,
            "totalCartDiscountAmount": bag(outcome.cart),
            "totalDuties": bag(0),
            "totalTax": bag(outcome.tax),
        })),
    )))
}

/// `returnCalculate`: what a return of the lines given would be, without making it. Lines
/// that are not of the order, or past what is left to return, are answered with a GraphQL error.
fn calculate(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let input = model::input_of(ctx.arg("input"));
    let state = r.state();
    let order_id = input
        .get("orderId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Some(order) = state
        .get(&order_id)
        .filter(|_| state.type_of(&order_id) == Some("Order"))
        .cloned()
    else {
        return Ok(Output::Null);
    };
    let currency = model::currency(state);
    let bag = |cents: i64| value::money_bag(cents, &currency);
    let mut outcome = Outcome::default();
    let mut lines = Vec::new();
    for (index, item) in array(&input, "returnLineItems").iter().enumerate() {
        let fulfillment = item
            .get("fulfillmentLineItemId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let of_order = state
            .get(fulfillment)
            .and_then(|line| text(line, "lineItem"))
            .is_some_and(|line| {
                gids(order.get("lineItems"))
                    .iter()
                    .any(|known| known == line)
            });
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if !of_order
            || quantity <= 0
            || quantity > super::returns::returnable_quantity(state, fulfillment)
        {
            return Err(FieldError::new(format!(
                "returnLineItems[{index}]: the fulfillment line item is not returnable in that quantity"
            )));
        }
        let percentage = item
            .get("restockingFee")
            .and_then(|fee| fee.get("percentage"))
            .and_then(Value::as_f64);
        let (subtotal, tax, before, fee) =
            outcome.add_line(state, &order, fulfillment, quantity, percentage);
        lines.push(json!({
            "id": null,
            "fulfillmentLineItem": fulfillment,
            "quantity": quantity,
            "subtotalBeforeOrderDiscountsSet": bag(before),
            "subtotalSet": bag(subtotal),
            "totalTaxSet": bag(tax),
            "restockingFee": percentage.map(|percentage| json!({
                "id": format!("gid://shopify/CalculatedRestockingFee/{}", value::uuid(&format!("{order_id}/{index}/fee"))),
                "percentage": percentage,
                "amountSet": bag(fee),
            })),
        }));
    }
    let mut errors = Vec::new();
    let exchanges = read_exchanges(
        state,
        &array(&input, "exchangeLineItems"),
        &["exchangeLineItems"],
        &mut errors,
    );
    if let Some(error) = errors.first() {
        return Err(FieldError::new(error.message.clone()));
    }
    let mut exchange_lines = Vec::new();
    for exchange in &exchanges {
        let (subtotal, tax) = outcome.add_exchange(
            state,
            &order,
            exchange.unit,
            exchange.quantity,
            exchange.discount,
        );
        let discounted_unit = if exchange.quantity == 0 {
            exchange.unit
        } else {
            exchange.unit - exchange.discount / exchange.quantity
        };
        exchange_lines.push(json!({
            "id": null,
            "variant": exchange.variant,
            "quantity": exchange.quantity,
            "originalUnitPriceSet": bag(exchange.unit),
            "discountedUnitPriceSet": bag(discounted_unit),
            "subtotalSet": bag(subtotal),
            "totalTaxSet": bag(tax),
            "calculatedDiscountAllocations": [],
        }));
    }
    let shipping_fee = input
        .get("returnShippingFee")
        .filter(|fee| !fee.is_null())
        .map(|fee| {
            json!({
                "id": format!("gid://shopify/CalculatedReturnShippingFee/{}", value::uuid(&format!("{order_id}/shipping-fee"))),
                "amountSet": bag(model::cents(fee.get("amount"))),
                "taxAmountSet": bag(0),
            })
        });
    let key: Vec<String> = lines
        .iter()
        .chain(&exchange_lines)
        .map(Value::to_string)
        .collect();
    Ok(Output::Object(Obj::built(
        "CalculatedReturn",
        value::object(json!({
            "id": format!("gid://shopify/CalculatedReturn/{}", value::uuid(&format!("{order_id}/{}", key.join("/")))),
            "returnLineItems": lines,
            "exchangeLineItems": exchange_lines,
            "returnShippingFee": shipping_fee,
        })),
    )))
}
