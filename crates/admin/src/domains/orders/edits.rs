//! Order edits: `orderEditBegin`, `orderEditAddVariant`, `orderEditAddCustomItem`,
//! `orderEditSetQuantity`, `orderEditAddLineItemDiscount`, `orderEditRemoveDiscount`,
//! `orderEditCommit`.
//!
//! As shopify.dev describes the flow (the reference of 2026-10 and the tutorial "Edit an
//! existing order", `apps/build/orders-fulfillment/order-management-apps/edit-orders`):
//!
//! - `orderEditBegin` makes a `CalculatedOrder` of the order (a record, with an
//!   `OrderEditSession` of the same number: the staging mutations take either id), its line
//!   items the order's with units left. Only the units that did not ship are editable.
//! - The staging mutations change the calculated order only: added variants and custom items,
//!   quantities (0 removes what can be removed), discounts on added line items. Each answers the
//!   calculated order and the calculated line item. Nothing fires.
//! - `orderEditCommit` applies them: added line items (their units committed in stock, on an
//!   open fulfillment order of the location), quantities raised, and the removals as a refund of
//!   restock type `CANCEL` (put back in stock when `restock`) or `NO_RESTOCK`, as Shopify
//!   records them. The tax follows (the store data's rate, else the order's). The order is
//!   `edited`; its financial status follows its new total. Events: `refunds/create` for the
//!   removals, `orders/edited`, `orders/updated` ("triggered whenever an order edit is
//!   completed", and a staff answer on community.shopify.dev).
//!
//! Messages: "The order cannot be edited." is quoted from community.shopify.com; the others are
//! lsf's wording. Shipping lines in an edit (`orderEditAddShippingLine` and the others) are not
//! simulated.
//!
//! The calculated line items keep what the commit needs besides their fields: the line item
//! they are of (`lsfLineItem`), the units that cannot be edited (`lsfShipped`), and for an
//! added one whether it is taxable and ships (`lsfTaxable`, `lsfRequiresShipping`) and what its
//! discount takes off (`lsfDiscount`, in cents). GraphQL never reads them.

use serde_json::{Map, Value, json};

use super::model::{self, field_cents, int, text};
use super::refunds::{self, RefundLine, RefundSpec};
use super::{LineDraft, TAX_RATE_SETTING};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, gid, parse_gid, value};
use crate::webhooks::payload::{
    EDIT_ADDITIONS, EDIT_COMMITTED_AT, EDIT_NOTIFY_CUSTOMER, EDIT_REMOVALS, EDIT_STAFF_NOTE,
};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("orderEditBegin", begin)
        .mutation("orderEditAddVariant", add_variant)
        .mutation("orderEditAddCustomItem", add_custom_item)
        .mutation("orderEditSetQuantity", set_quantity)
        .mutation("orderEditAddLineItemDiscount", add_line_item_discount)
        .mutation("orderEditRemoveDiscount", remove_discount)
        .mutation("orderEditCommit", commit);
}

fn number(id: &str) -> u64 {
    parse_gid(id).map_or(0, |(_, number)| number)
}

/// The cents of a calculated line item's field.
fn cents_of(line: &Map<String, Value>, field: &str) -> i64 {
    field_cents(line, field)
}

/// `orderEditBegin`.
fn begin(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let failed = |error: UserError| {
        Ok(payload(
            json!({"calculatedOrder": null, "orderEditSession": null}),
            &[error],
        ))
    };
    let state = mutation.state();
    let Some(order) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("Order"))
        .cloned()
    else {
        return failed(UserError::new(&["id"], "Order does not exist"));
    };
    if model::is_cancelled(&order) {
        return failed(UserError::new(&["id"], "The order cannot be edited."));
    }
    let currency = model::currency(state);
    let bag = |cents: i64| value::money_bag(cents, &currency);
    let mut lines = Vec::new();
    for line in gids(order.get("lineItems")) {
        let Some(item) = state.get(&line) else {
            continue;
        };
        let current = int(item, "currentQuantity");
        if current == 0 {
            continue;
        }
        let editable = int(item, "unfulfilledQuantity").min(current);
        let ordered = int(item, "quantity").max(1);
        let unit = field_cents(item, "discountedTotalSet") / ordered;
        lines.push(json!({
            "id": gid("CalculatedLineItem", number(&line)),
            "title": item.get("title"),
            "variantTitle": item.get("variantTitle"),
            "sku": item.get("sku"),
            "variant": item.get("variant"),
            "image": item.get("image"),
            "customAttributes": item.get("customAttributes").cloned().unwrap_or(json!([])),
            "quantity": current,
            "editableQuantity": editable,
            "editableQuantityBeforeChanges": editable,
            "originalUnitPriceSet": item.get("originalUnitPriceSet"),
            "discountedUnitPriceSet": bag(unit),
            "editableSubtotalSet": bag(unit * editable),
            "uneditableSubtotalSet": bag(unit * (current - editable)),
            "restockable": true,
            "restocking": false,
            "hasStagedLineItemDiscount": false,
            "stagedChanges": [],
            "calculatedDiscountAllocations": [],
            "discountAllocations": [],
            "lsfLineItem": line,
            "lsfShipped": current - editable,
        }));
    }
    let state = mutation.state_mut();
    let calculated = state.new_gid("CalculatedOrder");
    let session = gid("OrderEditSession", number(&calculated));
    state.insert(value::object(json!({
        "__typename": "OrderEditSession",
        "id": session,
    })));
    let name = text(&order, "name").unwrap_or_default().to_string();
    state.insert(value::object(json!({
        "__typename": "CalculatedOrder",
        "id": calculated,
        "originalOrder": id,
        "committed": false,
        "lineItems": lines,
        "addedLineItems": [],
        "addedDiscountApplications": [],
        "stagedChanges": [],
        "shippingLines": [],
        "createdAt": state.timestamp(),
        "notificationPreviewTitle": format!("Order {name} has been updated"),
        "notificationPreviewHtml": null,
    })));
    refresh_calculated(state, &calculated);
    Ok(payload(
        json!({"calculatedOrder": calculated, "orderEditSession": session}),
        &[],
    ))
}

/// The calculated order an edit mutation names (`CalculatedOrder/<n>` or `OrderEditSession/
/// <n>`), not committed yet.
fn calculated_order(state: &ShopState, id: &str) -> Result<String, UserError> {
    let calculated = match parse_gid(id) {
        Some(("CalculatedOrder", _)) => id.to_string(),
        Some(("OrderEditSession", n)) => gid("CalculatedOrder", n),
        _ => String::new(),
    };
    let Some(record) = state.get(&calculated) else {
        return Err(UserError::new(&["id"], "The order edit does not exist."));
    };
    if record.get("committed").and_then(Value::as_bool) == Some(true) {
        return Err(UserError::new(
            &["id"],
            "The order edit was already committed.",
        ));
    }
    Ok(calculated)
}

/// The payload of a staging mutation: the calculated order, its session, a calculated line
/// item when there is one.
fn staged(state: &ShopState, calculated: &str, line: Option<&str>, extra: Value) -> Value {
    let line = line.and_then(|line| calculated_line(state, calculated, line));
    let mut fields = json!({
        "calculatedOrder": calculated,
        "orderEditSession": gid("OrderEditSession", number(calculated)),
        "calculatedLineItem": line,
    });
    if let (Value::Object(fields), Value::Object(extra)) = (&mut fields, extra) {
        fields.extend(extra);
    }
    payload(fields, &[])
}

/// A calculated line item of a calculated order, by its id.
fn calculated_line(state: &ShopState, calculated: &str, line: &str) -> Option<Value> {
    state
        .get(calculated)?
        .get("lineItems")?
        .as_array()?
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(line))
        .cloned()
}

/// Changes a calculated line item in place (and in `addedLineItems`), then the totals.
fn change_line(
    state: &mut ShopState,
    calculated: &str,
    line: &str,
    change: impl Fn(&mut Map<String, Value>),
) {
    if let Some(record) = state.get_mut(calculated) {
        for field in ["lineItems", "addedLineItems"] {
            if let Some(Value::Array(items)) = record.get_mut(field) {
                for item in items.iter_mut() {
                    if item.get("id").and_then(Value::as_str) == Some(line)
                        && let Value::Object(item) = item
                    {
                        change(item);
                    }
                }
            }
        }
    }
    refresh_calculated(state, calculated);
}

/// The tax rate of an order's edits, in basis points: the store data's, else what the order's
/// tax is of its subtotal.
fn tax_rate_bps(state: &ShopState, order: &Record) -> i64 {
    let setting = state
        .setting(TAX_RATE_SETTING)
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if setting > 0 {
        return setting;
    }
    let subtotal = field_cents(order, "subtotalPriceSet");
    if subtotal <= 0 {
        0
    } else {
        model::share(field_cents(order, "totalTaxSet"), 10_000, subtotal)
    }
}

/// Recomputes a calculated order: each line's subtotals and staged changes, then the order's
/// quantity, subtotal, tax, total and outstanding amount.
fn refresh_calculated(state: &mut ShopState, calculated: &str) {
    let Some(record) = state.get(calculated).cloned() else {
        return;
    };
    let currency = model::currency(state);
    let bag = |cents: i64| value::money_bag(cents, &currency);
    let order_id = text(&record, "originalOrder")
        .unwrap_or_default()
        .to_string();
    let order = state.get(&order_id).cloned().unwrap_or_default();
    let rate = tax_rate_bps(state, &order);
    let mut lines: Vec<Value> = record
        .get("lineItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut changes = Vec::new();
    let (mut quantity, mut subtotal, mut taxable) = (0, 0, 0);
    for line in &mut lines {
        let Value::Object(line) = line else {
            continue;
        };
        let units = int(line, "quantity");
        let shipped = int(line, "lsfShipped");
        let unit = cents_of(line, "originalUnitPriceSet");
        let discount = line.get("lsfDiscount").and_then(Value::as_i64).unwrap_or(0);
        let added = line.get("lsfLineItem").is_none_or(Value::is_null);
        let discounted_unit = if added {
            if units == 0 {
                unit
            } else {
                unit - discount / units
            }
        } else {
            cents_of(line, "discountedUnitPriceSet")
        };
        let editable = units - shipped;
        let line_total = if added {
            unit * units - discount
        } else {
            discounted_unit * units
        };
        line.insert("editableQuantity".into(), json!(editable));
        line.insert("discountedUnitPriceSet".into(), bag(discounted_unit));
        line.insert(
            "editableSubtotalSet".into(),
            bag(line_total - discounted_unit * shipped),
        );
        line.insert(
            "uneditableSubtotalSet".into(),
            bag(discounted_unit * shipped),
        );
        let mut line_changes = Vec::new();
        if added {
            line_changes.push(match line.get("variant").filter(|variant| !variant.is_null()) {
                Some(variant) => json!({"__typename": "OrderStagedChangeAddVariant", "quantity": units, "variant": variant}),
                None => json!({
                    "__typename": "OrderStagedChangeAddCustomItem",
                    "title": line.get("title"),
                    "quantity": units,
                    "originalUnitPrice": value::money_v2(unit, &currency),
                }),
            });
            if let Some(application) = line
                .get("calculatedDiscountAllocations")
                .and_then(Value::as_array)
                .and_then(|allocations| allocations.first())
                .and_then(|allocation| allocation.get("discountApplication"))
            {
                line_changes.push(json!({
                    "__typename": "OrderStagedChangeAddLineItemDiscount",
                    "id": application.get("id"),
                    "description": application.get("description").and_then(Value::as_str).unwrap_or_default(),
                    "value": application.get("value"),
                }));
            }
        } else {
            let before = int(line, "editableQuantityBeforeChanges") + shipped;
            let item = line.get("lsfLineItem").cloned().unwrap_or(Value::Null);
            if units > before {
                line_changes.push(json!({"__typename": "OrderStagedChangeIncrementItem", "delta": units - before, "lineItem": item}));
            } else if units < before {
                line_changes.push(json!({
                    "__typename": "OrderStagedChangeDecrementItem",
                    "delta": before - units,
                    "lineItem": item,
                    "restock": line.get("restocking").and_then(Value::as_bool).unwrap_or(false),
                }));
            }
        }
        line.insert("stagedChanges".into(), json!(line_changes));
        changes.extend(line_changes);
        quantity += units;
        subtotal += line_total;
        let line_taxable = if added {
            line.get("lsfTaxable")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        } else {
            line.get("lsfLineItem")
                .and_then(Value::as_str)
                .and_then(|item| state.get(item))
                .and_then(|item| item.get("taxable"))
                .and_then(Value::as_bool)
                .unwrap_or(true)
        };
        if line_taxable {
            taxable += line_total;
        }
    }
    let cart = field_cents(&order, "cartDiscountAmountSet");
    let subtotal = subtotal - cart.min(subtotal);
    let tax = if order.get("taxExempt").and_then(Value::as_bool) == Some(true) {
        0
    } else {
        ((taxable - cart.min(taxable)) as f64 * rate as f64 / 10_000.0).round() as i64
    };
    let shipping = field_cents(&order, "currentShippingPriceSet");
    let total = subtotal + shipping + tax;
    let net = field_cents(&order, "netPaymentSet");
    let added: Vec<Value> = lines
        .iter()
        .filter(|line| line.get("lsfLineItem").is_none_or(Value::is_null))
        .cloned()
        .collect();
    let tax_lines: Vec<Value> = (tax > 0)
        .then(|| json!({"title": "Tax", "rate": rate as f64 / 10_000.0, "ratePercentage": rate as f64 / 100.0, "priceSet": bag(tax)}))
        .into_iter()
        .collect();
    if let Some(record) = state.get_mut(calculated) {
        record.insert("lineItems".into(), json!(lines));
        record.insert("addedLineItems".into(), json!(added));
        record.insert("stagedChanges".into(), json!(changes));
        record.insert("subtotalLineItemsQuantity".into(), json!(quantity));
        record.insert("subtotalPriceSet".into(), bag(subtotal));
        record.insert("cartDiscountAmountSet".into(), bag(cart));
        record.insert("taxLines".into(), json!(tax_lines));
        record.insert("totalPriceSet".into(), bag(total));
        record.insert("totalOutstandingSet".into(), bag(total - net));
    }
}

/// The quantity argument of an addition, checked.
fn positive_quantity(ctx: &FieldContext<'_>) -> Result<i64, UserError> {
    match ctx.arg_i64("quantity") {
        Some(quantity) if quantity > 0 => Ok(quantity),
        _ => Err(UserError::new(
            &["quantity"],
            "Quantity must be greater than 0",
        )),
    }
}

/// Refuses a line whose total would be past the bound of an amount (`value::MAX_MONEY_CENTS`),
/// so that the edit's sums of them hold. lsf's wording.
fn bounded_line(unit: i64, quantity: i64) -> Result<(), UserError> {
    match unit.checked_mul(quantity) {
        Some(total) if value::money_in_bounds(total) => Ok(()),
        _ => Err(UserError::new(
            &["quantity"],
            value::money_too_large("Total price"),
        )),
    }
}

/// Adds a calculated line item to a calculated order; returns its id.
fn add_line(state: &mut ShopState, calculated: &str, mut line: Map<String, Value>) -> String {
    let id = state.new_gid("CalculatedLineItem");
    line.insert("id".into(), json!(id));
    if let Some(Value::Array(items)) = state
        .get_mut(calculated)
        .and_then(|record| record.get_mut("lineItems"))
    {
        items.push(Value::Object(line));
    }
    refresh_calculated(state, calculated);
    id
}

/// The fields of an added calculated line item.
fn added_line(
    currency: &str,
    title: Value,
    variant: Value,
    unit: i64,
    quantity: i64,
    extra: Value,
) -> Map<String, Value> {
    let mut line = value::object(json!({
        "title": title,
        "variantTitle": null,
        "sku": null,
        "variant": variant,
        "image": null,
        "customAttributes": [],
        "quantity": quantity,
        "editableQuantity": quantity,
        "editableQuantityBeforeChanges": 0,
        "originalUnitPriceSet": value::money_bag(unit, currency),
        "restockable": false,
        "restocking": false,
        "hasStagedLineItemDiscount": false,
        "stagedChanges": [],
        "calculatedDiscountAllocations": [],
        "discountAllocations": [],
        "lsfLineItem": null,
        "lsfShipped": 0,
        "lsfDiscount": 0,
    }));
    if let Value::Object(extra) = extra {
        line.extend(extra);
    }
    line
}

/// `orderEditAddVariant`.
fn add_variant(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let failed = |error: UserError| {
        Ok(payload(
            json!({"calculatedOrder": null, "calculatedLineItem": null, "orderEditSession": null}),
            &[error],
        ))
    };
    let state = mutation.state();
    let calculated = match calculated_order(state, ctx.arg_str("id").unwrap_or_default()) {
        Ok(calculated) => calculated,
        Err(error) => return failed(error),
    };
    let quantity = match positive_quantity(ctx) {
        Ok(quantity) => quantity,
        Err(error) => return failed(error),
    };
    let variant = ctx.arg_str("variantId").unwrap_or_default().to_string();
    let Some(variant_record) = state
        .get(&variant)
        .filter(|_| state.type_of(&variant) == Some("ProductVariant"))
        .cloned()
    else {
        return failed(UserError::new(&["variantId"], "Variant does not exist"));
    };
    let on_order = state
        .get(&calculated)
        .and_then(|record| record.get("lineItems"))
        .and_then(Value::as_array)
        .is_some_and(|lines| {
            lines
                .iter()
                .any(|line| line.get("variant").and_then(Value::as_str) == Some(variant.as_str()))
        });
    if on_order && ctx.arg_bool("allowDuplicates") != Some(true) {
        return failed(UserError::new(
            &["variantId"],
            "The variant is already on the order. Set `allowDuplicates` to add it again.",
        ));
    }
    let product = text(&variant_record, "product")
        .and_then(|product| state.get(product))
        .cloned()
        .unwrap_or_default();
    let item = text(&variant_record, "inventoryItem")
        .and_then(|item| state.get(item))
        .cloned()
        .unwrap_or_default();
    let currency = model::currency(state);
    if let Err(error) = bounded_line(field_cents(&variant_record, "price"), quantity) {
        return failed(error);
    }
    let variant_title = text(&variant_record, "title").filter(|title| *title != "Default Title");
    let line = added_line(
        &currency,
        product.get("title").cloned().unwrap_or(Value::Null),
        json!(variant),
        field_cents(&variant_record, "price"),
        quantity,
        json!({
            "variantTitle": variant_title,
            "sku": variant_record.get("sku"),
            "image": variant_record.get("image"),
            "restockable": true,
            "lsfTaxable": variant_record.get("taxable").and_then(Value::as_bool).unwrap_or(true),
            "lsfRequiresShipping": item.get("requiresShipping").and_then(Value::as_bool).unwrap_or(true),
            "lsfLocation": ctx.arg_str("locationId"),
        }),
    );
    let state = mutation.state_mut();
    let id = add_line(state, &calculated, line);
    Ok(staged(state, &calculated, Some(&id), json!({})))
}

/// `orderEditAddCustomItem`.
fn add_custom_item(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| {
        Ok(payload(
            json!({"calculatedOrder": null, "calculatedLineItem": null, "orderEditSession": null}),
            &[error],
        ))
    };
    let calculated = match calculated_order(mutation.state(), ctx.arg_str("id").unwrap_or_default())
    {
        Ok(calculated) => calculated,
        Err(error) => return failed(error),
    };
    let quantity = match positive_quantity(ctx) {
        Ok(quantity) => quantity,
        Err(error) => return failed(error),
    };
    let title = ctx.arg_str("title").unwrap_or_default().trim().to_string();
    if title.is_empty() {
        return failed(UserError::new(&["title"], "Title can't be blank"));
    }
    let price = model::cents(ctx.arg("price"));
    if price < 0 {
        return failed(UserError::new(&["price"], "Price can't be negative"));
    }
    if let Err(error) = bounded_line(price, quantity) {
        return failed(error);
    }
    let currency = model::currency(mutation.state());
    let line = added_line(
        &currency,
        json!(title),
        Value::Null,
        price,
        quantity,
        json!({
            "lsfTaxable": ctx.arg_bool("taxable").unwrap_or(true),
            "lsfRequiresShipping": ctx.arg_bool("requiresShipping").unwrap_or(true),
            "lsfLocation": ctx.arg_str("locationId"),
        }),
    );
    let state = mutation.state_mut();
    let id = add_line(state, &calculated, line);
    Ok(staged(state, &calculated, Some(&id), json!({})))
}

/// `orderEditSetQuantity`.
fn set_quantity(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let failed = |error: UserError| {
        Ok(payload(
            json!({"calculatedOrder": null, "calculatedLineItem": null, "orderEditSession": null}),
            &[error],
        ))
    };
    let state = mutation.state();
    let calculated = match calculated_order(state, ctx.arg_str("id").unwrap_or_default()) {
        Ok(calculated) => calculated,
        Err(error) => return failed(error),
    };
    let line_id = ctx.arg_str("lineItemId").unwrap_or_default().to_string();
    let Some(line) = calculated_line(state, &calculated, &line_id) else {
        return failed(UserError::new(&["lineItemId"], "Line item does not exist"));
    };
    let quantity = ctx.arg_i64("quantity").unwrap_or(0);
    if quantity < 0 {
        return failed(UserError::new(&["quantity"], "Quantity cannot be negative"));
    }
    let shipped = line.get("lsfShipped").and_then(Value::as_i64).unwrap_or(0);
    if quantity < shipped {
        return failed(UserError::new(
            &["quantity"],
            "Quantity cannot be less than the quantity that was fulfilled",
        ));
    }
    let unit = line
        .as_object()
        .map_or(0, |line| cents_of(line, "originalUnitPriceSet"));
    if let Err(error) = bounded_line(unit, quantity) {
        return failed(error);
    }
    let restock = ctx.arg_bool("restock").unwrap_or(false);
    let state = mutation.state_mut();
    change_line(state, &calculated, &line_id, |line| {
        line.insert("quantity".into(), json!(quantity));
        line.insert("restocking".into(), json!(restock));
    });
    Ok(staged(state, &calculated, Some(&line_id), json!({})))
}

/// `orderEditAddLineItemDiscount`: a discount on an added line item.
fn add_line_item_discount(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| {
        Ok(payload(
            json!({"calculatedOrder": null, "calculatedLineItem": null, "orderEditSession": null, "addedDiscountStagedChange": null}),
            &[error],
        ))
    };
    let state = mutation.state();
    let calculated = match calculated_order(state, ctx.arg_str("id").unwrap_or_default()) {
        Ok(calculated) => calculated,
        Err(error) => return failed(error),
    };
    let line_id = ctx.arg_str("lineItemId").unwrap_or_default().to_string();
    let Some(line) = calculated_line(state, &calculated, &line_id) else {
        return failed(UserError::new(&["lineItemId"], "Line item does not exist"));
    };
    if line.get("lsfLineItem").is_some_and(|item| !item.is_null()) {
        return failed(UserError::new(
            &["lineItemId"],
            "A discount can only be added to a line item added in this edit",
        ));
    }
    let discount = model::input_of(ctx.arg("discount"));
    let currency = model::currency(state);
    let units = line.get("quantity").and_then(Value::as_i64).unwrap_or(0);
    let unit = line
        .as_object()
        .map_or(0, |line| cents_of(line, "originalUnitPriceSet"));
    let percent = discount.get("percentValue").and_then(Value::as_f64);
    let fixed = discount
        .get("fixedValue")
        .map(|value| model::cents(Some(value)));
    let (amount, value) = match (percent, fixed) {
        (Some(percent), None) if (0.0..=100.0).contains(&percent) => (
            ((unit * units) as f64 * percent / 100.0).round() as i64,
            json!({"__typename": "PricingPercentageValue", "percentage": percent}),
        ),
        (None, Some(fixed)) if fixed >= 0 => (
            fixed.saturating_mul(units).min(unit * units),
            json!({"__typename": "MoneyV2", "amount": value::decimal(fixed), "currencyCode": currency}),
        ),
        _ => {
            return failed(UserError::new(
                &["discount"],
                "The discount needs either a percent value from 0 to 100 or a fixed value",
            ));
        }
    };
    let state = mutation.state_mut();
    let application_id = state.new_gid("CalculatedManualDiscountApplication");
    let application = json!({
        "__typename": "CalculatedManualDiscountApplication",
        "id": application_id,
        "description": discount.get("description"),
        "value": value,
        "allocationMethod": "EACH",
        "appliedTo": "LINE",
        "targetSelection": "EXPLICIT",
        "targetType": "LINE_ITEM",
    });
    let allocation = json!({
        "allocatedAmountSet": value::money_bag(amount, &currency),
        "discountApplication": application,
    });
    change_line(state, &calculated, &line_id, |line| {
        line.insert("lsfDiscount".into(), json!(amount));
        line.insert("hasStagedLineItemDiscount".into(), json!(true));
        line.insert("calculatedDiscountAllocations".into(), json!([allocation]));
    });
    if let Some(Value::Array(applications)) = state
        .get_mut(&calculated)
        .and_then(|record| record.get_mut("addedDiscountApplications"))
    {
        applications.push(application.clone());
    }
    let change = json!({
        "id": application_id,
        "description": discount.get("description").and_then(Value::as_str).unwrap_or_default(),
        "value": application["value"],
    });
    Ok(staged(
        state,
        &calculated,
        Some(&line_id),
        json!({"addedDiscountStagedChange": change}),
    ))
}

/// `orderEditRemoveDiscount`: removes a discount added in the edit.
fn remove_discount(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| {
        Ok(payload(
            json!({"calculatedOrder": null, "orderEditSession": null}),
            &[error],
        ))
    };
    let state = mutation.state();
    let calculated = match calculated_order(state, ctx.arg_str("id").unwrap_or_default()) {
        Ok(calculated) => calculated,
        Err(error) => return failed(error.code("INVALID")),
    };
    let application = ctx
        .arg_str("discountApplicationId")
        .unwrap_or_default()
        .to_string();
    let line = state
        .get(&calculated)
        .and_then(|record| record.get("lineItems"))
        .and_then(Value::as_array)
        .and_then(|lines| {
            lines.iter().find(|line| {
                line.pointer("/calculatedDiscountAllocations/0/discountApplication/id")
                    .and_then(Value::as_str)
                    == Some(application.as_str())
            })
        })
        .and_then(|line| line.get("id").and_then(Value::as_str))
        .map(str::to_string);
    let Some(line) = line else {
        return failed(
            UserError::new(
                &["discountApplicationId"],
                "The discount does not exist on this edit",
            )
            .code("INVALID"),
        );
    };
    let state = mutation.state_mut();
    change_line(state, &calculated, &line, |line| {
        line.insert("lsfDiscount".into(), json!(0));
        line.insert("hasStagedLineItemDiscount".into(), json!(false));
        line.insert("calculatedDiscountAllocations".into(), json!([]));
    });
    if let Some(Value::Array(applications)) = state
        .get_mut(&calculated)
        .and_then(|record| record.get_mut("addedDiscountApplications"))
    {
        applications
            .retain(|known| known.get("id").and_then(Value::as_str) != Some(application.as_str()));
    }
    Ok(staged(state, &calculated, None, json!({})))
}

/// Adds units of a line item to a fulfillment order of the order that is still open (at
/// `location` when one is), or to a new one there.
fn add_to_fulfillment(
    state: &mut ShopState,
    order: &str,
    line: &str,
    quantity: i64,
    location: Option<&str>,
) {
    let fulfillment_orders = state
        .get(order)
        .map(|record| gids(record.get("fulfillmentOrders")))
        .unwrap_or_default();
    let open: Vec<String> = fulfillment_orders
        .iter()
        .filter(|fo| {
            matches!(
                state.get(fo).and_then(|record| text(record, "status")),
                Some("OPEN" | "IN_PROGRESS")
            )
        })
        .cloned()
        .collect();
    let target = open
        .iter()
        .find(|fo| location.is_none() || model::assigned_location(state, fo).as_deref() == location)
        .cloned();
    let existing = target.as_ref().and_then(|fo| {
        model::fulfillment_order_lines(state, order, line)
            .into_iter()
            .find(|(known, _)| known == fo)
            .map(|(_, fo_line)| fo_line)
    });
    if let Some(fo_line) = existing {
        if let Some(record) = state.get_mut(&fo_line) {
            let total = int(record, "totalQuantity");
            let remaining = int(record, "remainingQuantity");
            record.insert("totalQuantity".into(), json!(total + quantity));
            record.insert("remainingQuantity".into(), json!(remaining + quantity));
        }
        if let Some(fo) = &target {
            model::refresh_fulfillment_order(state, fo);
            state.touch(fo);
        }
        return;
    }
    let item = state.get(line).cloned().unwrap_or_default();
    let fo_line = state.new_gid("FulfillmentOrderLineItem");
    let inventory_item = model::inventory_item_of(state, &item);
    state.insert(value::object(json!({
        "__typename": "FulfillmentOrderLineItem",
        "id": fo_line,
        "lineItem": line,
        "totalQuantity": quantity,
        "remainingQuantity": quantity,
        "productTitle": item.get("title"),
        "variantTitle": item.get("variantTitle"),
        "sku": item.get("sku"),
        "vendor": item.get("vendor"),
        "variant": item.get("variant"),
        "inventoryItemId": inventory_item,
        "requiresShipping": item.get("requiresShipping"),
        "image": item.get("image"),
        "weight": item.get("weight"),
        "warnings": [],
    })));
    match target {
        Some(fo) => {
            model::push_key(state, &fo, "lineItems", &fo_line);
            model::refresh_fulfillment_order(state, &fo);
            state.touch(&fo);
        }
        None => {
            let Some(template) = fulfillment_orders.first().cloned() else {
                return;
            };
            let new = super::fulfillments::split_off(state, &template, &[], location, false);
            if let Some(record) = state.get_mut(&new) {
                record.insert("lineItems".into(), json!([fo_line]));
            }
            model::push_key(state, order, "fulfillmentOrders", &new);
        }
    }
}

/// `orderEditCommit`.
fn commit(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let failed = |error: UserError| {
        Ok(payload(
            json!({"order": null, "successMessages": null}),
            &[error],
        ))
    };
    let state = mutation.state();
    let calculated = match calculated_order(state, ctx.arg_str("id").unwrap_or_default()) {
        Ok(calculated) => calculated,
        Err(error) => return failed(error),
    };
    let record = state.get(&calculated).cloned().unwrap_or_default();
    let order = text(&record, "originalOrder")
        .unwrap_or_default()
        .to_string();
    let Some(order_record) = state.get(&order).cloned() else {
        return failed(UserError::new(&["id"], "Order does not exist"));
    };
    if model::is_cancelled(&order_record) {
        return failed(UserError::new(&["id"], "The order cannot be edited."));
    }
    if ctx.arg_bool("notifyCustomer") == Some(true) {
        mutation.note("orderEditCommit: lsf sends no email: `notifyCustomer` was ignored");
    }
    let staff_note = ctx.arg_str("staffNote").map(str::to_string);
    let lines: Vec<Map<String, Value>> = record
        .get("lineItems")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|line| line.as_object().cloned())
        .collect();

    let state = mutation.state_mut();
    let currency = model::currency(state);
    let now = state.timestamp();
    let captured = matches!(
        text(&order_record, "displayFinancialStatus"),
        Some("PAID" | "PARTIALLY_PAID" | "PARTIALLY_REFUNDED" | "REFUNDED")
    );
    let default_location = model::assigned_location(
        state,
        gids(order_record.get("fulfillmentOrders"))
            .first()
            .map_or("", String::as_str),
    );
    let mut additions: Vec<Value> = Vec::new();
    let mut removed: Vec<Value> = Vec::new();
    let mut removals = RefundSpec {
        note: staff_note.clone(),
        ..RefundSpec::default()
    };
    for line in &lines {
        let units = int(line, "quantity");
        match line.get("lsfLineItem").and_then(Value::as_str) {
            Some(item_id) => {
                let current = state
                    .get(item_id)
                    .map_or(0, |item| int(item, "currentQuantity"));
                if units > current {
                    let delta = units - current;
                    additions.push(json!({"id": item_id, "delta": delta}));
                    if let Some(item) = state.get_mut(item_id) {
                        let ordered = int(item, "quantity") + delta;
                        let unit = field_cents(item, "originalUnitPriceSet");
                        let discounted_unit = field_cents(item, "discountedUnitPriceSet");
                        item.insert("quantity".into(), json!(ordered));
                        item.insert(
                            "originalTotalSet".into(),
                            value::money_bag(unit * ordered, &currency),
                        );
                        item.insert(
                            "discountedTotalSet".into(),
                            value::money_bag(discounted_unit * ordered, &currency),
                        );
                    }
                    let location = model::line_location(state, &order, item_id);
                    add_to_fulfillment(state, &order, item_id, delta, location.as_deref());
                    model::commit_line(state, item_id, location.as_deref(), delta);
                } else if units < current {
                    removed.push(json!({"id": item_id, "delta": current - units}));
                    let restock = line.get("restocking").and_then(Value::as_bool) == Some(true);
                    removals.lines.push(RefundLine {
                        line: item_id.to_string(),
                        quantity: current - units,
                        restock_type: if restock { "CANCEL" } else { "NO_RESTOCK" }.into(),
                        location: None,
                    });
                }
            }
            None if units > 0 => {
                let variant = text(line, "variant").map(str::to_string);
                let variant_record = variant
                    .as_deref()
                    .and_then(|variant| state.get(variant))
                    .cloned();
                let product = variant_record
                    .as_ref()
                    .and_then(|variant| text(variant, "product"))
                    .map(str::to_string);
                let inventory_item = variant_record
                    .as_ref()
                    .and_then(|variant| text(variant, "inventoryItem"))
                    .map(str::to_string);
                let weight = inventory_item
                    .as_deref()
                    .and_then(|item| state.get(item))
                    .and_then(|item| item.get("measurement"))
                    .and_then(|measurement| measurement.get("weight"))
                    .cloned();
                let draft = LineDraft {
                    product: product.clone(),
                    variant: variant.clone(),
                    inventory_item: inventory_item.clone(),
                    title: text(line, "title").unwrap_or_default().to_string(),
                    variant_title: text(line, "variantTitle").map(str::to_string),
                    sku: text(line, "sku").map(str::to_string),
                    vendor: product
                        .as_deref()
                        .and_then(|product| state.get(product))
                        .and_then(|product| text(product, "vendor"))
                        .map(str::to_string),
                    quantity: units as u32,
                    unit_price: cents_of(line, "originalUnitPriceSet"),
                    discount: line.get("lsfDiscount").and_then(Value::as_i64).unwrap_or(0),
                    requires_shipping: line
                        .get("lsfRequiresShipping")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    taxable: line
                        .get("lsfTaxable")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    gift_card: false,
                    image: line.get("image").filter(|image| !image.is_null()).cloned(),
                    weight,
                    grams: 0,
                    attributes: Vec::new(),
                };
                let id = state.new_gid("LineItem");
                let mut item =
                    super::line_item_record(&id, &draft, &currency, draft.quantity, captured, &now);
                if let Some(allocations) = line
                    .get("calculatedDiscountAllocations")
                    .and_then(Value::as_array)
                {
                    item.insert(
                        "discountAllocations".into(),
                        json!(allocations
                            .iter()
                            .map(|allocation| json!({
                                "allocatedAmountSet": allocation.get("allocatedAmountSet"),
                                "discountApplication": {
                                    "__typename": "ManualDiscountApplication",
                                    "title": allocation.pointer("/discountApplication/description").and_then(Value::as_str).unwrap_or("Discount"),
                                    "description": allocation.pointer("/discountApplication/description"),
                                    "value": allocation.pointer("/discountApplication/value"),
                                    "allocationMethod": "EACH",
                                    "targetSelection": "EXPLICIT",
                                    "targetType": "LINE_ITEM",
                                    "index": 0,
                                },
                            }))
                            .collect::<Vec<_>>()),
                    );
                }
                state.insert(item);
                additions.push(json!({"id": id, "delta": units}));
                model::push_key(state, &order, "lineItems", &id);
                let location = line
                    .get("lsfLocation")
                    .and_then(Value::as_str)
                    .filter(|location| state.type_of(location) == Some("Location"))
                    .map(str::to_string)
                    .or_else(|| default_location.clone());
                add_to_fulfillment(state, &order, &id, units, location.as_deref());
                model::commit_line(state, &id, location.as_deref(), units);
            }
            None => {}
        }
    }
    if !removals.lines.is_empty() {
        refunds::create_refund(state, &order, &removals);
    }
    // The tax of the edited order is the calculated order's.
    let wanted_tax = record
        .get("taxLines")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|line| field_cents(line.as_object().unwrap_or(&Map::new()), "priceSet"))
        .sum::<i64>();
    let current = state.get(&order).cloned().unwrap_or_default();
    let refunded_tax =
        field_cents(&current, "totalTaxSet") - field_cents(&current, "currentTotalTaxSet");
    let tax = wanted_tax + refunded_tax;
    if let Some(item) = state.get_mut(&order) {
        item.insert("totalTaxSet".into(), value::money_bag(tax, &currency));
        let tax_lines: Vec<Value> = (tax > 0)
            .then(|| json!({"title": "Tax", "priceSet": value::money_bag(tax, &currency)}))
            .into_iter()
            .collect();
        item.insert("taxLines".into(), json!(tax_lines));
        item.insert("edited".into(), json!(true));
    }
    let notify = ctx.arg_bool("notifyCustomer") == Some(true);
    if let Some(record) = state.get_mut(&calculated) {
        record.insert("committed".into(), json!(true));
        record.insert(EDIT_COMMITTED_AT.into(), json!(now));
        record.insert(EDIT_NOTIFY_CUSTOMER.into(), json!(notify));
        record.insert(EDIT_STAFF_NOTE.into(), json!(staff_note));
        record.insert(EDIT_ADDITIONS.into(), json!(additions));
        record.insert(EDIT_REMOVALS.into(), json!(removed));
    }
    model::settle_financial_status(state, &order);
    let current_tax = state
        .get(&order)
        .map_or(0, |record| field_cents(record, "currentTotalTaxSet"));
    if let Some(item) = state.get_mut(&order) {
        let current_lines: Vec<Value> = (current_tax > 0)
            .then(|| json!({"title": "Tax", "priceSet": value::money_bag(current_tax, &currency)}))
            .into_iter()
            .collect();
        item.insert("currentTaxLines".into(), json!(current_lines));
    }
    state.touch(&order);
    // `orders/edited` is about the edit: its payload is the committed calculated order's.
    state.emit("orders/edited", &calculated);
    state.emit("orders/updated", &order);
    Ok(payload(json!({"order": order, "successMessages": []}), &[]))
}
