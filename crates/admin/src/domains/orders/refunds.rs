//! Refunds: `refundCreate`, `Order.suggestedRefund`, and the refund records `orderCancel`, order
//! edits and `returnProcess` make the same way.
//!
//! As shopify.dev's reference of 2026-10 (`mutations/refundCreate`, `enums/
//! RefundLineItemRestockType`, `objects/SuggestedRefund`) describes them:
//!
//! - A refund records line items and shipping taken back, and moves money only through its
//!   `transactions` ("Create a refund for an item in an order" passes `transactions: []` and
//!   answers `totalRefundedSet` 0.0). Each transaction is a `REFUND` against a sale or a capture
//!   of the order, through its gateway.
//! - Restock types: `CANCEL` takes units that did not ship and puts them back in stock;
//!   `RETURN` puts back units that shipped; `NO_RESTOCK` takes units that did not ship first and
//!   leaves the stock as it is; `LEGACY_RESTOCK` is "not accepted when creating new refunds". A
//!   unit that did not ship leaves its fulfillment order.
//! - Amounts: a line refunds its price after discounts (the order's discount spread over the
//!   lines), and its share of the order's tax.
//! - `refundMethods.storeCreditRefund` gives money back as store credit: a `REFUND` through
//!   the store credit gateway, against the order's payments, credited to the customer's
//!   account (`store_credit`).
//! - Events: `order_transactions/create` for each transaction, `refunds/create` ("whenever a new
//!   refund is created without errors on an order, independent from the movement of money"),
//!   then `orders/updated`, which a staff answer on community.shopify.dev says fires with it.
//!
//! Messages: "Shipping must have amount less than or equal to remaining shipping" and "either
//! refund line items or refund duties or transactions or refund methods must be present" are
//! quoted from real answers on community.shopify.dev; "cannot refund more items than were
//! purchased" and "cannot remove more than the fulfillable quantity" from community.shopify.com
//! (REST-style, the same validations). The others are lsf's wording: shopify.dev does not show
//! them.

use serde_json::{Value, json};

use super::model::{self, NewTransaction, array, field_cents, int, text};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry.mutation("refundCreate", refund_create).field(
        "Order",
        "suggestedRefund",
        suggested_refund,
    );
}

/// A line of a refund to record.
#[derive(Clone, Debug)]
pub struct RefundLine {
    pub line: String,
    pub quantity: i64,
    /// `CANCEL`, `RETURN` or `NO_RESTOCK`.
    pub restock_type: String,
    pub location: Option<String>,
}

/// A refund to record: its lines, its shipping, the money it gives back.
#[derive(Clone, Debug, Default)]
pub struct RefundSpec {
    pub lines: Vec<RefundLine>,
    /// The shipping refunded, in cents.
    pub shipping: i64,
    /// `(amount, gateway, parent)` of each `REFUND` transaction.
    pub transactions: Vec<(i64, Option<String>, Option<String>)>,
    pub note: Option<String>,
    pub processed_at: Option<String>,
    /// The return the refund is of.
    pub return_id: Option<String>,
    /// `(amount, expiresAt)` of each refund to the customer's store credit: a `REFUND` through
    /// the store credit gateway, against the order's payments, credited to its account.
    pub store_credit: Vec<(i64, Option<String>)>,
    /// Whether the refund is of a cancellation: the store credit's event is then
    /// `ORDER_CANCELLATION`, else `ORDER_REFUND`.
    pub cancellation: bool,
}

/// What a line refunds for `quantity` units: `(unit price after all discounts, subtotal, tax)`.
pub fn line_amounts(
    state: &ShopState,
    order: &Record,
    line: &str,
    quantity: i64,
) -> (i64, i64, i64) {
    let Some(item) = state.get(line) else {
        return (0, 0, 0);
    };
    let ordered = int(item, "quantity").max(1);
    let lines_total: i64 = gids(order.get("lineItems"))
        .iter()
        .filter_map(|id| state.get(id))
        .map(|item| field_cents(item, "discountedTotalSet"))
        .sum();
    let discounted = field_cents(item, "discountedTotalSet");
    let cart = field_cents(order, "cartDiscountAmountSet");
    let after_cart = if lines_total == 0 {
        discounted
    } else {
        discounted - model::share(cart, discounted, lines_total)
    };
    let unit = after_cart / ordered;
    let subtotal = model::share(after_cart, quantity, ordered);
    let order_subtotal = field_cents(order, "subtotalPriceSet");
    let tax = field_cents(order, "totalTaxSet");
    let line_tax =
        if order_subtotal <= 0 || item.get("taxable").and_then(Value::as_bool) == Some(false) {
            0
        } else {
            model::share(tax, subtotal, order_subtotal)
        };
    (unit, subtotal, line_tax)
}

/// The shipping of an order that was not refunded yet, in cents.
pub fn refundable_shipping(order: &Record) -> i64 {
    field_cents(order, "totalShippingPriceSet") - field_cents(order, "totalRefundedShippingSet")
}

/// Records a refund on an order: the `Refund` (its line items and shipping lines inline), the
/// units taken off the fulfillment orders, the stock put back, the `REFUND` transactions. Emits
/// `order_transactions/create` for each transaction and `refunds/create`; the caller settles the
/// order's status and emits `orders/updated`. Returns the GID of the refund.
pub fn create_refund(state: &mut ShopState, order: &str, spec: &RefundSpec) -> String {
    let currency = model::currency(state);
    let bag = |cents: i64| value::money_bag(cents, &currency);
    let now = state.timestamp();
    let Some(order_record) = state.get(order).cloned() else {
        return String::new();
    };
    let mut refund_lines = Vec::new();
    for line in &spec.lines {
        let (unit, subtotal, tax) = line_amounts(state, &order_record, &line.line, line.quantity);
        let location = line
            .location
            .clone()
            .or_else(|| model::line_location(state, order, &line.line));
        let restocked = line.restock_type != "NO_RESTOCK";
        // Units that did not ship leave their fulfillment orders: all of a `CANCEL`, as many as
        // there are of a `NO_RESTOCK`.
        // Units of a return shipped: they leave no fulfillment order.
        if line.restock_type != "RETURN" && spec.return_id.is_none() {
            model::take_unfulfilled(state, order, &line.line, line.quantity);
        }
        if restocked {
            model::restock_line(
                state,
                &line.line,
                location.as_deref(),
                line.quantity,
                line.restock_type == "RETURN",
            );
        }
        let id = state.new_gid("RefundLineItem");
        refund_lines.push(json!({
            "id": id,
            "lineItem": line.line,
            "quantity": line.quantity,
            "restockType": line.restock_type,
            "restocked": restocked,
            "location": if restocked { json!(location) } else { Value::Null },
            "priceSet": bag(unit),
            "subtotalSet": bag(subtotal),
            "totalTaxSet": bag(tax),
        }));
    }
    let mut shipping_lines = Vec::new();
    if spec.shipping > 0 {
        let shipping_line = array(&order_record, "shippingLines")
            .into_iter()
            .next()
            .unwrap_or(Value::Null);
        let id = state.new_gid("RefundShippingLine");
        shipping_lines.push(json!({
            "id": id,
            "shippingLine": shipping_line,
            "subtotalAmountSet": bag(spec.shipping),
            "taxAmountSet": bag(0),
        }));
    }
    let mut transactions = Vec::new();
    let mut refunded = 0;
    for (amount, gateway, parent) in &spec.transactions {
        let id = model::add_transaction(
            state,
            order,
            &NewTransaction {
                kind: "REFUND",
                amount: *amount,
                gateway: gateway.as_deref(),
                parent: parent.as_deref(),
                manual: gateway.as_deref().is_none_or(|gateway| gateway == "manual"),
                test: order_record.get("test").and_then(Value::as_bool) == Some(true),
            },
        );
        refunded += amount;
        transactions.push(id);
    }
    for (amount, expires_at) in &spec.store_credit {
        // Against the order's payments, as a refund to the original methods is.
        let parent = state
            .get(order)
            .map(|record| model::refundable_payments(state, record))
            .unwrap_or_default()
            .into_iter()
            .find(|(_, left)| *left > 0)
            .map(|(id, _)| id);
        let id = model::add_transaction(
            state,
            order,
            &NewTransaction {
                kind: "REFUND",
                amount: *amount,
                gateway: Some(super::store_credit::GATEWAY),
                parent: parent.as_deref(),
                manual: false,
                test: order_record.get("test").and_then(Value::as_bool) == Some(true),
            },
        );
        let event = if spec.cancellation {
            "ORDER_CANCELLATION"
        } else {
            "ORDER_REFUND"
        };
        super::store_credit::refund(state, order, &id, *amount, expires_at.as_deref(), event);
        refunded += amount;
        transactions.push(id);
    }
    let id = state.new_gid("Refund");
    state.insert(value::object(json!({
        "__typename": "Refund",
        "id": id,
        "order": order,
        "note": spec.note,
        "createdAt": now,
        "processedAt": spec.processed_at.clone().unwrap_or_else(|| now.clone()),
        "updatedAt": now,
        "refundLineItems": refund_lines,
        "refundShippingLines": shipping_lines,
        "transactions": transactions,
        "totalRefundedSet": bag(refunded),
        "return": spec.return_id,
        "duties": [],
        "orderAdjustments": [],
        "staffMember": null,
    })));
    model::push_key(state, order, "refunds", &id);
    if let Some(return_id) = &spec.return_id {
        model::push_key(state, return_id, "refunds", &id);
        for transaction in &transactions {
            model::push_key(state, return_id, "transactions", transaction);
        }
    }
    model::refresh_order(state, order);
    state.emit("refunds/create", &id);
    id
}

/// The refunds to store credit of `refundMethods` (`[RefundMethodInput]`, each a
/// `storeCreditRefund`), checked: `(cents, expiresAt)`.
pub fn store_credit_refunds(
    state: &ShopState,
    order: &Record,
    methods: &[Value],
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Vec<(i64, Option<String>)> {
    let mut refunds = Vec::new();
    for (index, method) in methods.iter().enumerate() {
        let index = index.to_string();
        let Some(input) = method.get("storeCreditRefund").and_then(Value::as_object) else {
            continue;
        };
        let at = [path, &[index.as_str(), "storeCreditRefund"]].concat();
        match super::store_credit::read_refund(state, order, input, &at) {
            Ok(refund) => refunds.push(refund),
            Err(error) => errors.push(error),
        }
    }
    refunds
}

/// The units of a line that may still be refunded, and of those, how many did not ship.
fn refundable_units(state: &ShopState, order: &str, line: &str) -> (i64, i64) {
    let current = state
        .get(line)
        .map_or(0, |item| int(item, "currentQuantity"));
    let unfulfilled = state
        .get(line)
        .map_or(0, |item| int(item, "unfulfilledQuantity"));
    let _ = order;
    (current, unfulfilled.min(current))
}

/// The `RefundLineItemInput`s of a refund (or a suggested one), checked against the order.
fn refund_lines(
    state: &ShopState,
    order: &str,
    items: &[Value],
    path: &str,
    errors: &mut Vec<UserError>,
) -> Vec<RefundLine> {
    let order_lines = state
        .get(order)
        .map(|record| gids(record.get("lineItems")))
        .unwrap_or_default();
    let mut lines: Vec<RefundLine> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let index = index.to_string();
        let line = item
            .get("lineItemId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        let restock_type = item
            .get("restockType")
            .and_then(Value::as_str)
            .unwrap_or("NO_RESTOCK")
            .to_string();
        if !order_lines.contains(&line) {
            // lsf's wording: shopify.dev does not show this error.
            errors.push(UserError::new(
                &[path, &index, "lineItemId"],
                "Line item does not exist",
            ));
            continue;
        }
        if quantity <= 0 {
            // lsf's wording.
            errors.push(UserError::new(
                &[path, &index, "quantity"],
                "Quantity must be greater than 0",
            ));
            continue;
        }
        if restock_type == "LEGACY_RESTOCK" {
            errors.push(UserError::new(
                &[path, &index, "restockType"],
                "Restock type LEGACY_RESTOCK is not accepted when creating new refunds",
            ));
            continue;
        }
        let taken: i64 = lines
            .iter()
            .filter(|known| known.line == line)
            .map(|known| known.quantity)
            .sum();
        let taken_unfulfilled: i64 = lines
            .iter()
            .filter(|known| known.line == line && known.restock_type != "RETURN")
            .map(|known| known.quantity)
            .sum();
        let (current, unfulfilled) = refundable_units(state, order, &line);
        if quantity + taken > current {
            errors.push(UserError::new(
                &[path, &index, "quantity"],
                "cannot refund more items than were purchased",
            ));
            continue;
        }
        match restock_type.as_str() {
            "CANCEL" if quantity + taken_unfulfilled > unfulfilled => {
                errors.push(UserError::new(
                    &[path, &index, "quantity"],
                    "cannot remove more than the fulfillable quantity",
                ));
                continue;
            }
            "RETURN" if quantity > current - unfulfilled => {
                // lsf's wording.
                errors.push(UserError::new(
                    &[path, &index, "quantity"],
                    "cannot return more than the fulfilled quantity",
                ));
                continue;
            }
            _ => {}
        }
        let location = item
            .get("locationId")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(location) = &location
            && state.type_of(location) != Some("Location")
        {
            errors.push(UserError::new(
                &[path, &index, "locationId"],
                "Location does not exist",
            ));
            continue;
        }
        lines.push(RefundLine {
            line,
            quantity,
            restock_type,
            location,
        });
    }
    lines
}

/// `refundCreate`.
fn refund_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = model::input_of(ctx.arg("input"));
    let order = input
        .get("orderId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let failed = |errors: &[UserError]| Ok(payload(json!({"refund": null, "order": null}), errors));
    let Some(order_record) = state
        .get(&order)
        .filter(|_| state.type_of(&order) == Some("Order"))
    else {
        // lsf's wording.
        return failed(&[UserError::new(&["orderId"], "Order does not exist")]);
    };
    let items = array(&input, "refundLineItems");
    let transactions = array(&input, "transactions");
    let duties = array(&input, "refundDuties");
    let methods = array(&input, "refundMethods");
    if items.is_empty() && transactions.is_empty() && duties.is_empty() && methods.is_empty() {
        return failed(&[UserError::new(
            &[],
            "either refund line items or refund duties or transactions or refund methods must be present",
        )]);
    }
    let mut errors = Vec::new();
    let lines = refund_lines(state, &order, &items, "refundLineItems", &mut errors);
    let shipping_input = model::input_of(input.get("shipping"));
    let remaining_shipping = refundable_shipping(order_record);
    let shipping = if shipping_input.get("fullRefund").and_then(Value::as_bool) == Some(true) {
        remaining_shipping
    } else {
        model::cents(shipping_input.get("amount"))
    };
    if shipping > remaining_shipping || shipping < 0 {
        errors.push(UserError::new(
            &["shipping"],
            "Shipping must have amount less than or equal to remaining shipping",
        ));
    }
    let over_refunding = input.get("allowOverRefunding").and_then(Value::as_bool) == Some(true);
    let payments = model::refundable_payments(state, order_record);
    let mut moves = Vec::new();
    for (index, transaction) in transactions.iter().enumerate() {
        let index = index.to_string();
        let field = |name: &str| transaction.get(name).and_then(Value::as_str);
        let amount = model::cents(transaction.get("amount"));
        if field("kind") != Some("REFUND") {
            // lsf's wording; Shopify answers "The authorized transaction has already been
            // captured" for a sale or capture refunded with another kind.
            errors.push(UserError::new(
                &["transactions", &index, "kind"],
                "Transaction kind must be REFUND",
            ));
            continue;
        }
        if field("orderId").is_some_and(|id| id != order) {
            errors.push(UserError::new(
                &["transactions", &index, "orderId"],
                "Transaction must belong to the order being refunded",
            ));
            continue;
        }
        let parent = match field("parentId") {
            Some(parent) => match payments.iter().find(|(id, _)| id == parent) {
                Some(payment) => Some(payment.clone()),
                None => {
                    errors.push(UserError::new(
                        &["transactions", &index, "parentId"],
                        "Parent transaction does not exist",
                    ));
                    continue;
                }
            },
            None => payments.first().cloned(),
        };
        if amount <= 0 {
            errors.push(UserError::new(
                &["transactions", &index, "amount"],
                "Amount must be greater than 0",
            ));
            continue;
        }
        if !over_refunding && parent.as_ref().is_none_or(|(_, left)| amount > *left) {
            // lsf's wording.
            errors.push(UserError::new(
                &["transactions", &index, "amount"],
                "Amount exceeds the refundable amount of the transaction",
            ));
            continue;
        }
        let gateway = field("gateway")
            .map(str::to_string)
            .or_else(|| model::payment_gateway(state, order_record));
        moves.push((amount, gateway, parent.map(|(id, _)| id)));
    }
    let store_credit = store_credit_refunds(
        state,
        order_record,
        &methods,
        &["refundMethods"],
        &mut errors,
    );
    let left: i64 = payments.iter().map(|(_, left)| left).sum::<i64>()
        - moves.iter().map(|(amount, _, _)| amount).sum::<i64>();
    let credited: i64 = store_credit.iter().map(|(amount, _)| amount).sum();
    if credited > 0 && !over_refunding && credited > left {
        // lsf's wording, as the transactions' above.
        errors.push(UserError::new(
            &["refundMethods"],
            "Amount exceeds the refundable amount of the order",
        ));
    }
    if !duties.is_empty() {
        mutation.note("refundCreate: lsf does not simulate duties: `refundDuties` was ignored");
    }
    if !errors.is_empty() {
        return failed(&errors);
    }
    let spec = RefundSpec {
        lines,
        shipping,
        transactions: moves,
        note: input
            .get("note")
            .and_then(Value::as_str)
            .map(str::to_string),
        processed_at: input
            .get("processedAt")
            .and_then(Value::as_str)
            .map(str::to_string),
        return_id: None,
        store_credit,
        cancellation: false,
    };
    let state = mutation.state_mut();
    let refund = create_refund(state, &order, &spec);
    model::settle_financial_status(state, &order);
    model::order_updated(state, &order);
    Ok(payload(json!({"refund": refund, "order": order}), &[]))
}

/// `Order.suggestedRefund`: what a refund of the lines and shipping asked would give back, and
/// through which payments.
fn suggested_refund(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(order_record) = r.map_of(object) else {
        return r.generic(ctx, object);
    };
    let state = r.state();
    let order = text(order_record, "id").unwrap_or_default().to_string();
    let currency = model::currency(state);
    let bag = |cents: i64| value::money_bag(cents, &currency);
    let full = ctx.arg_bool("suggestFullRefund") == Some(true);
    let items: Vec<Value> = if full {
        gids(order_record.get("lineItems"))
            .iter()
            .filter_map(|line| {
                let item = state.get(line)?;
                let current = int(item, "currentQuantity");
                (current > 0).then(|| json!({"lineItemId": line, "quantity": current}))
            })
            .collect()
    } else {
        ctx.arg("refundLineItems")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut ignored = Vec::new();
    let lines = refund_lines(state, &order, &items, "refundLineItems", &mut ignored);
    let mut refund_lines = Vec::new();
    let (mut subtotal, mut tax) = (0, 0);
    for line in &lines {
        let (unit, line_subtotal, line_tax) =
            line_amounts(state, order_record, &line.line, line.quantity);
        subtotal += line_subtotal;
        tax += line_tax;
        refund_lines.push(json!({
            "lineItem": line.line,
            "quantity": line.quantity,
            "restockType": line.restock_type,
            "restocked": false,
            "location": line.location,
            "priceSet": bag(unit),
            "subtotalSet": bag(line_subtotal),
            "totalTaxSet": bag(line_tax),
        }));
    }
    let remaining_shipping = refundable_shipping(order_record);
    let shipping = if full || ctx.arg_bool("refundShipping") == Some(true) {
        remaining_shipping
    } else {
        model::cents(ctx.arg("shippingAmount")).min(remaining_shipping)
    };
    let (suggested, maximum) =
        suggested_transactions(state, order_record, subtotal + tax + shipping);
    let amount = (subtotal + tax + shipping).min(maximum);
    let cart = field_cents(order_record, "cartDiscountAmountSet");
    let value = json!({
        "amountSet": bag(amount),
        "maximumRefundableSet": bag(maximum),
        "subtotalSet": bag(subtotal),
        "discountedSubtotalSet": bag(subtotal),
        "totalTaxSet": bag(tax),
        "totalCartDiscountAmountSet": bag(cart),
        "totalDutiesSet": bag(0),
        "refundDuties": [],
        "refundLineItems": refund_lines,
        "shipping": {
            "amountSet": bag(shipping),
            "maximumRefundableSet": bag(remaining_shipping),
            "taxSet": bag(0),
        },
        "suggestedRefundMethods": [],
        "suggestedTransactions": suggested,
    });
    Ok(Output::Object(Obj::built(
        "SuggestedRefund",
        value::object(value),
    )))
}

/// The suggested transactions of a refund of `amount`, over the order's payments in order, and
/// the most they can refund.
pub(super) fn suggested_transactions(
    state: &ShopState,
    order: &Record,
    amount: i64,
) -> (Vec<Value>, i64) {
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
