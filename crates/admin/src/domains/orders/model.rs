//! What the records of an order derive from one another, and the changes the mutations of the
//! family share: amounts, the transactions that move money, the inventory an order commits or
//! gives back.
//!
//! The records keep their derived values (an order's current totals, a line item's current and
//! unfulfilled quantities, a fulfillment order's status), as the seed writes them. A mutation
//! changes the records it is about (a refund, a fulfillment, a fulfillment order's line items),
//! then runs [`refresh_order`], which recomputes the rest the way the seed computed it:
//!
//! - a line item's `currentQuantity` is its `quantity` less what refunds took (removals by an
//!   order edit are refunds of restock type `CANCEL`, as Shopify records them); its
//!   `unfulfilledQuantity` is what its fulfillment order line items still have to fulfill;
//! - the order's `current…` amounts are its amounts less what the refunds took back; its total is
//!   the subtotal, the shipping and the tax, as the seed counts it;
//! - the money received, refunded and capturable is the sum of its successful transactions;
//!   `totalOutstandingSet` is the current total less the net payment (negative when the
//!   customer is owed money, as shopify.dev describes the field);
//! - `displayFulfillmentStatus` follows the fulfillments and the fulfillment orders.
//!
//! `displayFinancialStatus` is not recomputed there: the store data gives statuses the money
//! alone cannot tell (a `partially_refunded` order without a refund), which an unrelated change
//! must not overwrite. [`settle_financial_status`] recomputes it after a change of money.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::domains::catalog::{self, gids};
use crate::domains::customers;
use crate::state::{Record, ShopState, value};

/// The financial statuses whose money was taken (`captured` in the seed).
const CAPTURED: [&str; 4] = ["PAID", "PARTIALLY_PAID", "PARTIALLY_REFUNDED", "REFUNDED"];

/// The currency of the shop: every amount of an order is in it.
pub fn currency(state: &ShopState) -> String {
    state
        .shop()
        .and_then(|shop| shop.get("currencyCode"))
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string()
}

/// The key of an order presented in another currency than the shop's ([`Presentment`]).
pub const PRESENTMENT: &str = "lsfPresentment";

/// The currency an order is presented in, when it is not the shop's (an order a subscription
/// contract in a presentment currency bills), and the rate from the shop's currency to it in
/// basis points (`markets::rate_bps`: a market's manual rate, else 1.0). The family computes
/// every amount in the shop's currency, as `shopMoney`; `presentmentMoney` is that amount
/// converted at the rate ([`apply_presentment`]). Kept on the order under [`PRESENTMENT`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presentment {
    pub currency: String,
    pub rate_bps: i64,
}

impl Presentment {
    /// The presentment of an order, `None` for an order in the shop's currency.
    pub fn of(order: &Record) -> Option<Presentment> {
        let kept = order.get(PRESENTMENT)?;
        Some(Presentment {
            currency: kept.get("currency")?.as_str()?.to_string(),
            rate_bps: kept.get("rateBps")?.as_i64().filter(|rate| *rate > 0)?,
        })
    }

    pub fn to_value(&self) -> Value {
        json!({"currency": self.currency, "rateBps": self.rate_bps})
    }

    /// An amount of the presentment currency in the shop's, in cents, rounded half away from
    /// zero.
    pub fn to_shop(&self, cents: i64) -> i64 {
        rounded_ratio(i128::from(cents) * 10_000, i128::from(self.rate_bps.max(1)))
    }

    /// An amount of the shop's currency in the presentment currency, in cents.
    pub fn from_shop(&self, cents: i64) -> i64 {
        rounded_ratio(i128::from(cents) * i128::from(self.rate_bps), 10_000)
    }
}

/// `numerator / denominator` (positive), rounded half away from zero, within `i64`.
fn rounded_ratio(numerator: i128, denominator: i128) -> i64 {
    let rounded = (numerator + numerator.signum() * denominator / 2) / denominator;
    i64::try_from(rounded).unwrap_or(if rounded < 0 { i64::MIN } else { i64::MAX })
}

/// Rewrites the `presentmentMoney` of every `MoneyBag` of an order presented in another
/// currency ([`Presentment`]) from its `shopMoney`: on the order and on the records it holds
/// (its line items, transactions, refunds and their lines, fulfillments, fulfillment orders,
/// returns, …, followed from the order two records deep). [`refresh_order`] runs it, so that
/// every flow of the family, which builds its amounts in the shop's currency, leaves the order
/// in its presentment currency. Writes only what differs; nothing for an order in the shop's
/// currency.
pub fn apply_presentment(state: &mut ShopState, order: &str) {
    let Some(presentment) = state.get(order).and_then(Presentment::of) else {
        return;
    };
    fn present(value: &mut Value, presentment: &Presentment) -> bool {
        match value {
            Value::Object(map) => {
                if let (Some(shop), true) =
                    (map.get("shopMoney"), map.contains_key("presentmentMoney"))
                {
                    let cents = cents(shop.get("amount"));
                    let converted =
                        value::money_v2(presentment.from_shop(cents), &presentment.currency);
                    if map.get("presentmentMoney") != Some(&converted) {
                        map.insert("presentmentMoney".into(), converted);
                        return true;
                    }
                    return false;
                }
                let mut changed = false;
                for value in map.values_mut() {
                    changed |= present(value, presentment);
                }
                changed
            }
            Value::Array(values) => {
                let mut changed = false;
                for value in values {
                    changed |= present(value, presentment);
                }
                changed
            }
            _ => false,
        }
    }
    fn references(record: &Record, into: &mut Vec<String>) {
        fn walk(value: &Value, into: &mut Vec<String>) {
            match value {
                Value::String(text) if text.starts_with("gid://shopify/") => {
                    if !into.contains(text) {
                        into.push(text.clone());
                    }
                }
                Value::Array(values) => values.iter().for_each(|value| walk(value, into)),
                _ => {}
            }
        }
        for (key, value) in record {
            // What the order holds, not what it points at (its customer, its location).
            if !matches!(key.as_str(), "customer" | "purchasingEntity" | "app" | "id") {
                walk(value, into);
            }
        }
    }
    let mut keys = vec![order.to_string()];
    let mut depth = 0;
    let mut frontier = vec![order.to_string()];
    while depth < 2 && !frontier.is_empty() {
        let mut next = Vec::new();
        for key in &frontier {
            if let Some(record) = state.get(key) {
                references(record, &mut next);
            }
        }
        next.retain(|key| {
            !keys.contains(key)
                && state.get(key).is_some_and(|record| {
                    // A record of the order: its own (`order`), or one of its line items'.
                    text(record, "order") == Some(order)
                        || matches!(
                            state.type_of(key),
                            Some(
                                "LineItem"
                                    | "RefundLineItem"
                                    | "FulfillmentLineItem"
                                    | "FulfillmentOrderLineItem"
                                    | "ReturnLineItem"
                                    | "UnverifiedReturnLineItem"
                                    | "OrderTransaction"
                                    | "Refund"
                                    | "Fulfillment"
                                    | "FulfillmentOrder"
                                    | "Return"
                            )
                        )
                })
        });
        keys.extend(next.iter().cloned());
        frontier = next;
        depth += 1;
    }
    for key in keys {
        let Some(mut record) = state.get(&key).cloned() else {
            continue;
        };
        let mut changed = false;
        for (field, value) in record.iter_mut() {
            if !field.starts_with("lsf") {
                changed |= present(value, &presentment);
            }
        }
        if changed && let Some(target) = state.get_mut(&key) {
            *target = record;
        }
    }
}

/// A decimal amount (`"19.99"`, `"20"`, `"0.125"`) in cents, rounded half away from zero.
/// `None` when it is not a number.
pub fn parse_cents(text: &str) -> Option<i64> {
    let text = text.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (units, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if (units.is_empty() && fraction.is_empty())
        || !units
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let units: i64 = if units.is_empty() {
        0
    } else {
        units.parse().ok()?
    };
    let digit = |index: usize| {
        fraction
            .as_bytes()
            .get(index)
            .map_or(0, |byte| i64::from(byte - b'0'))
    };
    // `None` past what 64 bits of cents hold, rather than an overflow.
    let cents = units
        .checked_mul(100)?
        .checked_add(digit(0) * 10 + digit(1) + i64::from(digit(2) >= 5))?;
    Some(if negative { -cents } else { cents })
}

/// The cents of an amount as the records and the inputs write it: a `MoneyBag` (its shop
/// money), a `MoneyV2` or `MoneyInput` (its `amount`), a `Money` or `Decimal` string, a number.
/// 0 when there is none.
pub fn cents(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Object(map)) => match map.get("shopMoney") {
            Some(shop) => cents(Some(shop)),
            None => cents(map.get("amount")),
        },
        Some(Value::String(text)) => parse_cents(text).unwrap_or(0),
        Some(Value::Number(number)) => number
            .as_f64()
            .map_or(0, |amount| (amount * 100.0).round() as i64),
        _ => 0,
    }
}

/// `amount × part / whole`, the share of an amount (a discount spread over lines, the tax of a
/// part of the subtotal), computed wide so that the product of two amounts does not overflow; 0
/// when `whole` is 0.
pub fn share(amount: i64, part: i64, whole: i64) -> i64 {
    if whole == 0 {
        return 0;
    }
    let share = i128::from(amount) * i128::from(part) / i128::from(whole);
    i64::try_from(share).unwrap_or(if share < 0 { i64::MIN } else { i64::MAX })
}

/// A `MoneyInput` argument: its cents and its currency, `None` when it is not given.
pub fn money_input(value: Option<&Value>) -> Option<(i64, Option<String>)> {
    let value = value.filter(|value| !value.is_null())?;
    Some((
        cents(Some(value)),
        value
            .get("currencyCode")
            .and_then(Value::as_str)
            .map(str::to_string),
    ))
}

/// The cents of a field of a record.
pub fn field_cents(record: &Record, field: &str) -> i64 {
    cents(record.get(field))
}

/// A string field of a record.
pub fn text<'r>(record: &'r Record, field: &str) -> Option<&'r str> {
    record.get(field).and_then(Value::as_str)
}

/// The number of a GID (`gid://shopify/Order/7` is 7), 0 when it is not one.
pub fn number(gid: &str) -> u64 {
    crate::state::parse_gid(gid).map_or(0, |(_, number)| number)
}

/// An integer field of a record, 0 when it has none.
pub fn int(record: &Record, field: &str) -> i64 {
    match record.get(field) {
        Some(Value::Number(number)) => number.as_i64().unwrap_or(0),
        Some(Value::String(text)) => text.parse().unwrap_or(0),
        _ => 0,
    }
}

/// The items of an array field: GIDs or inline objects.
pub fn array(record: &Record, field: &str) -> Vec<Value> {
    record
        .get(field)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Appends a key to an array field of a record (made if missing).
pub fn push_key(state: &mut ShopState, record: &str, field: &str, key: &str) {
    if let Some(record) = state.get_mut(record) {
        let items = record
            .entry(field.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(items) = items
            && !items.iter().any(|item| item.as_str() == Some(key))
        {
            items.push(json!(key));
        }
    }
}

/// Emits `orders/paid` for an order whose financial status is `PAID`.
pub fn emit_if_paid(state: &mut ShopState, order: &str) {
    if state
        .get(order)
        .and_then(|record| text(record, "displayFinancialStatus"))
        == Some("PAID")
    {
        state.emit("orders/paid", order);
    }
}

/// Whether an order is cancelled.
pub fn is_cancelled(order: &Record) -> bool {
    order.get("cancelledAt").is_some_and(|at| !at.is_null())
}

/// The order of a record that names one in `order` (a fulfillment order, a fulfillment, a
/// refund, a return), when it names an order of the shop.
pub fn order_of(state: &ShopState, record: &str) -> Option<String> {
    let order = state.get(record)?.get("order")?.as_str()?;
    (state.type_of(order) == Some("Order")).then(|| order.to_string())
}

/// The fulfillments of a line item that shipped: `(fulfillment line item GID, quantity)`, over
/// the order's successful fulfillments.
fn fulfilled_lines(state: &ShopState, order: &str, line: &str) -> Vec<(String, i64)> {
    let Some(record) = state.get(order) else {
        return Vec::new();
    };
    gids(record.get("fulfillments"))
        .iter()
        .filter_map(|fulfillment| state.get(fulfillment))
        .filter(|fulfillment| text(fulfillment, "status") == Some("SUCCESS"))
        .flat_map(|fulfillment| gids(fulfillment.get("fulfillmentLineItems")))
        .filter_map(|id| {
            let item = state.get(&id)?;
            (text(item, "lineItem") == Some(line)).then(|| (id.clone(), int(item, "quantity")))
        })
        .collect()
}

/// How many units of a line item shipped.
fn fulfilled_quantity(state: &ShopState, order: &str, line: &str) -> i64 {
    fulfilled_lines(state, order, line)
        .iter()
        .map(|(_, quantity)| quantity)
        .sum()
}

/// The fulfillment order line items of a line item, with their fulfillment order, over the
/// order's fulfillment orders: `(fulfillment order, its line item)`.
pub fn fulfillment_order_lines(
    state: &ShopState,
    order: &str,
    line: &str,
) -> Vec<(String, String)> {
    let Some(record) = state.get(order) else {
        return Vec::new();
    };
    gids(record.get("fulfillmentOrders"))
        .iter()
        .flat_map(|fulfillment_order| {
            let lines = super::line_items(state, fulfillment_order);
            lines
                .into_iter()
                .filter(|id| {
                    state
                        .get(id)
                        .is_some_and(|item| text(item, "lineItem") == Some(line))
                })
                .map(|id| (fulfillment_order.clone(), id))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Takes `quantity` unfulfilled units of a line item off its fulfillment orders (a removal, a
/// refund of units that did not ship): from the remaining quantity of each of its fulfillment
/// order line items, the open fulfillment orders first. Returns what it could take.
pub fn take_unfulfilled(state: &mut ShopState, order: &str, line: &str, quantity: i64) -> i64 {
    let mut left = quantity;
    let mut lines = fulfillment_order_lines(state, order, line);
    // Open fulfillment orders first, cancelled and closed ones last.
    lines.sort_by_key(|(fulfillment_order, _)| {
        match state
            .get(fulfillment_order)
            .and_then(|fo| text(fo, "status"))
        {
            Some("OPEN" | "IN_PROGRESS" | "ON_HOLD" | "SCHEDULED") => 0,
            _ => 1,
        }
    });
    let mut touched = Vec::new();
    for (fulfillment_order, id) in lines {
        if left == 0 {
            break;
        }
        let Some(item) = state.get_mut(&id) else {
            continue;
        };
        let remaining = int(item, "remainingQuantity");
        let taken = remaining.min(left);
        if taken <= 0 {
            continue;
        }
        item.insert("remainingQuantity".into(), json!(remaining - taken));
        let total = int(item, "totalQuantity");
        item.insert("totalQuantity".into(), json!((total - taken).max(0)));
        left -= taken;
        touched.push(fulfillment_order);
    }
    for fulfillment_order in touched {
        refresh_fulfillment_order(state, &fulfillment_order);
    }
    quantity - left
}

/// The status a fulfillment order's line items and fulfillments give it: `CLOSED` once nothing
/// remains, `IN_PROGRESS` once part of it shipped, `OPEN` otherwise. A fulfillment order on
/// hold, scheduled, incomplete or cancelled keeps its status until nothing remains of it.
pub fn refresh_fulfillment_order(state: &mut ShopState, fulfillment_order: &str) {
    let Some(record) = state.get(fulfillment_order) else {
        return;
    };
    let remaining: i64 = gids(record.get("lineItems"))
        .iter()
        .filter_map(|id| state.get(id))
        .map(|item| int(item, "remainingQuantity"))
        .sum();
    let shipped = gids(record.get("fulfillments")).iter().any(|fulfillment| {
        state
            .get(fulfillment)
            .is_some_and(|fulfillment| text(fulfillment, "status") == Some("SUCCESS"))
    });
    let status = text(record, "status").unwrap_or("OPEN");
    let new = match status {
        "CANCELLED" | "INCOMPLETE" => status,
        _ if remaining == 0 => "CLOSED",
        "OPEN" | "IN_PROGRESS" | "CLOSED"
            if shipped
                || crate::domains::logistics::fulfillment_orders::progress_reported(record) =>
        {
            "IN_PROGRESS"
        }
        "OPEN" | "IN_PROGRESS" | "CLOSED" => "OPEN",
        other => other,
    }
    .to_string();
    if Some(new.as_str()) != text(record, "status") {
        if let Some(record) = state.get_mut(fulfillment_order) {
            record.insert("status".into(), json!(new));
        }
        state.touch(fulfillment_order);
    }
}

/// What the refunds of an order took back.
#[derive(Default)]
struct Refunded {
    /// Units, by line item.
    quantities: HashMap<String, i64>,
    subtotal: i64,
    tax: i64,
    shipping: i64,
    /// Whether a refund put units back in stock.
    restocked: bool,
    /// What refunds gave back beyond the value of what they took back: a refund of money alone.
    /// Shopify records it as an order adjustment (`REFUND_DISCREPANCY`), which takes it off the
    /// current total.
    discrepancy: i64,
}

fn refunded(state: &ShopState, order: &Record) -> Refunded {
    let mut refunded = Refunded::default();
    for refund in gids(order.get("refunds"))
        .iter()
        .filter_map(|refund| state.get(refund))
    {
        let mut value = 0;
        for line in array(refund, "refundLineItems") {
            let Some(line) = line.as_object() else {
                continue;
            };
            if let Some(item) = text(line, "lineItem") {
                *refunded.quantities.entry(item.to_string()).or_default() += int(line, "quantity");
            }
            refunded.subtotal += field_cents(line, "subtotalSet");
            refunded.tax += field_cents(line, "totalTaxSet");
            value += field_cents(line, "subtotalSet") + field_cents(line, "totalTaxSet");
            refunded.restocked |= line.get("restocked").and_then(Value::as_bool) == Some(true);
        }
        for line in array(refund, "refundShippingLines") {
            if let Some(line) = line.as_object() {
                refunded.shipping += field_cents(line, "subtotalAmountSet");
                refunded.tax += field_cents(line, "taxAmountSet");
                value += field_cents(line, "subtotalAmountSet") + field_cents(line, "taxAmountSet");
            }
        }
        refunded.discrepancy += (field_cents(refund, "totalRefundedSet") - value).max(0);
    }
    refunded
}

/// What the refunds of an order gave back beyond the value of what they took back: Shopify's
/// `REFUND_DISCREPANCY` adjustment, `Order.refundDiscrepancySet`.
pub fn refund_discrepancy(state: &ShopState, order: &Record) -> i64 {
    refunded(state, order).discrepancy
}

/// What the transactions of an order moved: `(received, refunded, capturable, voided)`.
/// Received is what sales and captures took; capturable what authorizations hold that was
/// neither captured nor voided.
fn money_moved(state: &ShopState, order: &Record) -> (i64, i64, i64, bool) {
    let transactions: Vec<&Record> = gids(order.get("transactions"))
        .iter()
        .filter_map(|transaction| state.get(transaction))
        .filter(|transaction| text(transaction, "status") == Some("SUCCESS"))
        .collect();
    let (mut received, mut refunded, mut voided) = (0, 0, false);
    for transaction in &transactions {
        let amount = field_cents(transaction, "amountSet");
        match text(transaction, "kind") {
            Some("SALE" | "CAPTURE") => received += amount,
            Some("REFUND") => refunded += amount,
            Some("VOID") => voided = true,
            _ => {}
        }
    }
    let capturable = transactions
        .iter()
        .filter(|transaction| text(transaction, "kind") == Some("AUTHORIZATION"))
        .map(|authorization| capturable_of(&transactions, authorization))
        .sum();
    (received, refunded, capturable, voided)
}

/// What an authorization still holds: its amount less its captures, nothing once voided or
/// captured for the last time.
fn capturable_of(transactions: &[&Record], authorization: &Record) -> i64 {
    let id = text(authorization, "id");
    let children: Vec<&&Record> = transactions
        .iter()
        .filter(|transaction| text(transaction, "parentTransaction") == id)
        .collect();
    if children
        .iter()
        .any(|child| text(child, "kind") == Some("VOID"))
        || authorization
            .get("multiCapturable")
            .and_then(Value::as_bool)
            == Some(false)
            && children
                .iter()
                .any(|child| text(child, "kind") == Some("CAPTURE"))
    {
        return 0;
    }
    let captured: i64 = children
        .iter()
        .filter(|child| text(child, "kind") == Some("CAPTURE"))
        .map(|child| field_cents(child, "amountSet"))
        .sum();
    (field_cents(authorization, "amountSet") - captured).max(0)
}

/// What an authorization of an order still holds, by its GID: what `orderCapture` may take.
pub fn capturable_amount(state: &ShopState, order: &Record, authorization: &str) -> i64 {
    let transactions: Vec<&Record> = gids(order.get("transactions"))
        .iter()
        .filter_map(|transaction| state.get(transaction))
        .filter(|transaction| text(transaction, "status") == Some("SUCCESS"))
        .collect();
    state
        .get(authorization)
        .map_or(0, |record| capturable_of(&transactions, record))
}

/// Recomputes what an order derives from its line items, refunds, fulfillments, fulfillment
/// orders, transactions and returns (see the module's documentation), and the counters of its
/// customer and of the company location it was placed for. Every mutation of the family runs it after changing an order's records.
pub fn refresh_order(state: &mut ShopState, order: &str) {
    let Some(record) = state.get(order).cloned() else {
        return;
    };
    let currency = currency(state);
    let bag = |cents: i64| value::money_bag(cents, &currency);
    let refunded = refunded(state, &record);
    let financial = text(&record, "displayFinancialStatus").unwrap_or("PENDING");
    let captured = CAPTURED.contains(&financial);
    let cancelled = is_cancelled(&record);

    // The remaining units of each line item, over its fulfillment orders.
    let mut remaining: HashMap<String, i64> = HashMap::new();
    let fulfillment_orders = gids(record.get("fulfillmentOrders"));
    let mut fulfillment_statuses: Vec<String> = Vec::new();
    for fulfillment_order in fulfillment_orders.iter().filter_map(|id| state.get(id)) {
        let mut left = 0;
        for item in gids(fulfillment_order.get("lineItems"))
            .iter()
            .filter_map(|id| state.get(id))
        {
            if let Some(line) = text(item, "lineItem") {
                *remaining.entry(line.to_string()).or_default() += int(item, "remainingQuantity");
            }
            left += int(item, "remainingQuantity");
        }
        if left > 0 {
            fulfillment_statuses.push(
                text(fulfillment_order, "status")
                    .unwrap_or("OPEN")
                    .to_string(),
            );
        }
    }

    let lines = gids(record.get("lineItems"));
    let (mut lines_total, mut line_discounts) = (0, 0);
    let (mut quantity, mut current_quantity, mut unfulfilled_total, mut fulfilled_total) =
        (0, 0, 0, 0);
    let mut current_line_discounts = 0;
    for line in &lines {
        let fulfilled = fulfilled_quantity(state, order, line);
        let Some(item) = state.get_mut(line) else {
            continue;
        };
        let ordered = int(item, "quantity");
        let current = (ordered - refunded.quantities.get(line).copied().unwrap_or(0)).max(0);
        let unfulfilled = remaining.get(line).copied().unwrap_or(0).min(current);
        let original = field_cents(item, "originalTotalSet");
        let discounted = field_cents(item, "discountedTotalSet");
        let discount = field_cents(item, "totalDiscountSet");
        let share = |total: i64, units: i64| {
            if ordered == 0 {
                0
            } else {
                total * units / ordered
            }
        };
        item.insert("currentQuantity".into(), json!(current));
        item.insert("unfulfilledQuantity".into(), json!(unfulfilled));
        item.insert(
            "refundableQuantity".into(),
            json!(if captured { current } else { 0 }),
        );
        item.insert(
            "unfulfilledOriginalTotalSet".into(),
            bag(share(original, unfulfilled)),
        );
        item.insert(
            "unfulfilledDiscountedTotalSet".into(),
            bag(share(discounted, unfulfilled)),
        );
        lines_total += discounted;
        line_discounts += discount;
        current_line_discounts += share(discount, current);
        quantity += ordered;
        current_quantity += current;
        unfulfilled_total += unfulfilled;
        fulfilled_total += fulfilled;
    }

    let cart_discount = field_cents(&record, "cartDiscountAmountSet");
    let subtotal = lines_total - cart_discount;
    let shipping = field_cents(&record, "totalShippingPriceSet");
    let shipping_original: i64 = array(&record, "shippingLines")
        .iter()
        .filter_map(Value::as_object)
        .filter(|line| line.get("isRemoved").and_then(Value::as_bool) != Some(true))
        .map(|line| field_cents(line, "originalPriceSet"))
        .sum();
    let shipping_discount = (shipping_original - shipping).max(0);
    let tax = field_cents(&record, "totalTaxSet");
    let total = subtotal + shipping + tax;
    let current_subtotal = subtotal - refunded.subtotal;
    let current_shipping = shipping - refunded.shipping;
    let current_tax = tax - refunded.tax;
    let current_total = current_subtotal + current_shipping + current_tax - refunded.discrepancy;
    let (received, refunded_money, capturable, _) = money_moved(state, &record);
    let net = received - refunded_money;
    let weight = int(&record, "totalWeight");
    let current_weight = if quantity == 0 {
        0
    } else {
        weight * current_quantity / quantity
    };

    let display_fulfillment = if unfulfilled_total == 0 {
        if fulfilled_total > 0 {
            "FULFILLED"
        } else if refunded.restocked {
            "RESTOCKED"
        } else {
            "UNFULFILLED"
        }
    } else if fulfilled_total > 0 {
        "PARTIALLY_FULFILLED"
    } else if !fulfillment_statuses.is_empty()
        && fulfillment_statuses
            .iter()
            .all(|status| status == "ON_HOLD")
    {
        "ON_HOLD"
    } else if !fulfillment_statuses.is_empty()
        && fulfillment_statuses
            .iter()
            .all(|status| status == "SCHEDULED")
    {
        "SCHEDULED"
    } else {
        "UNFULFILLED"
    };
    let return_statuses: Vec<String> = gids(record.get("returns"))
        .iter()
        .filter_map(|id| state.get(id))
        .filter_map(|record| text(record, "status").map(str::to_string))
        .collect();
    let has = |status: &str| return_statuses.iter().any(|known| known == status);
    let return_status = if has("OPEN") {
        "IN_PROGRESS"
    } else if has("REQUESTED") {
        "RETURN_REQUESTED"
    } else if has("CLOSED") {
        "RETURNED"
    } else {
        "NO_RETURN"
    };

    let Some(record) = state.get_mut(order) else {
        return;
    };
    let fields = json!({
        "subtotalLineItemsQuantity": quantity,
        "currentSubtotalLineItemsQuantity": current_quantity,
        "subtotalPriceSet": bag(subtotal),
        "currentSubtotalPriceSet": bag(current_subtotal),
        "currentShippingPriceSet": bag(current_shipping),
        "currentTotalTaxSet": bag(current_tax),
        "totalDiscountsSet": bag(line_discounts + cart_discount + shipping_discount),
        "currentTotalDiscountsSet": bag(current_line_discounts + cart_discount + shipping_discount),
        "totalPriceSet": bag(total),
        "currentTotalPriceSet": bag(current_total),
        "totalReceivedSet": bag(received),
        "netPaymentSet": bag(net),
        "totalRefundedSet": bag(refunded_money),
        "totalRefundedShippingSet": bag(refunded.shipping),
        "totalOutstandingSet": bag(current_total - net),
        "totalCapturableSet": bag(capturable),
        "currentTotalWeight": current_weight.to_string(),
        "displayFulfillmentStatus": display_fulfillment,
        "fullyPaid": captured && received >= total,
        "unpaid": matches!(financial, "PENDING" | "AUTHORIZED" | "EXPIRED" | "VOIDED"),
        "capturable": capturable > 0 && !cancelled,
        "canMarkAsPaid": matches!(financial, "PENDING" | "PARTIALLY_PAID") && !cancelled,
        "refundable": captured && financial != "REFUNDED",
        "restockable": !cancelled,
        "fulfillable": !cancelled && unfulfilled_total > 0,
        "returnStatus": return_status,
    });
    if let Value::Object(fields) = fields {
        record.extend(fields);
    }
    let customer = record
        .get("customer")
        .and_then(Value::as_str)
        .map(str::to_string);
    let location = customers::order_company_location(record).map(str::to_string);
    if let Some(customer) = customer {
        customers::refresh_customer(state, &customer);
    }
    if let Some(location) = location {
        customers::refresh_company_location(state, &location);
    }
    if let Some(terms) = state
        .get(order)
        .and_then(|record| text(record, "paymentTerms"))
        .map(str::to_string)
    {
        super::payments::refresh_terms(state, &terms);
    }
    apply_presentment(state, order);
}

/// Recomputes `displayFinancialStatus` from the money of an order, after a change of money (a
/// payment, a capture, a refund, an edit of its total): refunded in full or in part once money
/// went back; paid in full or in part once it was taken; authorized while an authorization
/// holds money; voided once one was voided; paid when it costs nothing. An order none of these
/// apply to keeps its pending or expired status. Then [`refresh_order`] again, for what the
/// status implies.
pub fn settle_financial_status(state: &mut ShopState, order: &str) {
    refresh_order(state, order);
    let Some(record) = state.get(order) else {
        return;
    };
    let (received, refunded, capturable, voided) = money_moved(state, record);
    let total = field_cents(record, "currentTotalPriceSet");
    let current = text(record, "displayFinancialStatus").unwrap_or("PENDING");
    let status = if refunded > 0 && refunded >= received {
        "REFUNDED"
    } else if refunded > 0 {
        "PARTIALLY_REFUNDED"
    } else if received > 0 && received >= total {
        "PAID"
    } else if received > 0 {
        "PARTIALLY_PAID"
    } else if capturable > 0 {
        "AUTHORIZED"
    } else if voided {
        "VOIDED"
    } else if total <= 0 {
        "PAID"
    } else if matches!(current, "EXPIRED" | "VOIDED") {
        current
    } else {
        "PENDING"
    }
    .to_string();
    if let Some(record) = state.get_mut(order) {
        record.insert("displayFinancialStatus".into(), json!(status));
    }
    refresh_order(state, order);
}

/// A transaction to record on an order.
pub struct NewTransaction<'a> {
    /// `SALE`, `CAPTURE`, `AUTHORIZATION`, `REFUND`, `VOID`, …
    pub kind: &'a str,
    pub amount: i64,
    pub gateway: Option<&'a str>,
    pub parent: Option<&'a str>,
    /// Whether the money moved through a manual gateway (`orderMarkAsPaid`, a cash payment).
    pub manual: bool,
    pub test: bool,
}

/// Records a successful transaction on an order: an `OrderTransaction` in `transactions`, with
/// `order_transactions/create`, and the tender transaction of the money it moved, with
/// `tender_transactions/create` (`super::payments::record_tender`). Returns its GID. The order's
/// amounts are left to the caller's [`settle_financial_status`].
pub fn add_transaction(state: &mut ShopState, order: &str, transaction: &NewTransaction) -> String {
    add_transaction_with_status(state, order, transaction, "SUCCESS")
}

/// [`add_transaction`] in a status (`SUCCESS`, `PENDING`, `FAILURE`, `ERROR`): only a
/// successful one moves money, and so has a tender transaction.
pub fn add_transaction_with_status(
    state: &mut ShopState,
    order: &str,
    transaction: &NewTransaction,
    status: &str,
) -> String {
    let currency = currency(state);
    let now = state.timestamp();
    let id = state.new_gid("OrderTransaction");
    let gateway = transaction.gateway.unwrap_or("manual");
    state.insert(value::object(json!({
        "__typename": "OrderTransaction",
        "id": id,
        "kind": transaction.kind,
        "status": status,
        "amountSet": value::money_bag(transaction.amount, &currency),
        "gateway": gateway,
        "formattedGateway": formatted_gateway(gateway),
        "order": order,
        "parentTransaction": transaction.parent,
        "test": transaction.test,
        "manualPaymentGateway": transaction.manual,
        "manuallyCapturable": transaction.kind == "AUTHORIZATION",
        "multiCapturable": false,
        "createdAt": now,
        "processedAt": now,
        "fees": [],
    })));
    push_key(state, order, "transactions", &id);
    state.emit("order_transactions/create", &id);
    if let Some(tender) = super::payments::record_tender(state, &id) {
        state.emit("tender_transactions/create", &tender);
    }
    id
}

/// A gateway as the admin shows it: `manual` is "Manual", `bogus` "Bogus", others capitalized
/// word by word.
fn formatted_gateway(gateway: &str) -> String {
    gateway
        .split(['_', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The gateway of an order's first sale, capture or authorization: what a refund goes back
/// through.
pub fn payment_gateway(state: &ShopState, order: &Record) -> Option<String> {
    gids(order.get("transactions"))
        .iter()
        .filter_map(|id| state.get(id))
        .find(|transaction| {
            matches!(
                text(transaction, "kind"),
                Some("SALE" | "CAPTURE" | "AUTHORIZATION")
            )
        })
        .and_then(|transaction| text(transaction, "gateway").map(str::to_string))
}

/// The successful sales and captures of an order, with what each may still refund: the parents
/// a refund of its money goes against, in order.
pub fn refundable_payments(state: &ShopState, order: &Record) -> Vec<(String, i64)> {
    let transactions: Vec<&Record> = gids(order.get("transactions"))
        .iter()
        .filter_map(|id| state.get(id))
        .filter(|transaction| text(transaction, "status") == Some("SUCCESS"))
        .collect();
    transactions
        .iter()
        .filter(|transaction| matches!(text(transaction, "kind"), Some("SALE" | "CAPTURE")))
        .map(|payment| {
            let id = text(payment, "id").unwrap_or_default().to_string();
            let refunded: i64 = transactions
                .iter()
                .filter(|transaction| {
                    text(transaction, "kind") == Some("REFUND")
                        && text(transaction, "parentTransaction") == Some(id.as_str())
                })
                .map(|refund| field_cents(refund, "amountSet"))
                .sum();
            (id, (field_cents(payment, "amountSet") - refunded).max(0))
        })
        .collect()
}

/// The inventory level of an item at a location, `None` when it is not stocked there; its first
/// level when no location is given. A location given is never another: the units of an order
/// or a transfer at a location move there or nowhere.
fn inventory_level(state: &ShopState, item: &str, location: Option<&str>) -> Option<String> {
    let levels = gids(state.get(item)?.get("inventoryLevels"));
    let Some(location) = location else {
        return levels.first().cloned();
    };
    levels
        .into_iter()
        .find(|level| text_of(state, level, "location") == Some(location))
}

/// A text field of a record, by key.
fn text_of<'s>(state: &'s ShopState, key: &str, field: &str) -> Option<&'s str> {
    state.get(key).and_then(|record| text(record, field))
}

/// Changes the quantities of an inventory item at a location (`[("available", -2),
/// ("committed", 2)]`), as an order that commits, ships or restocks units does: a tracked item
/// only, at its level at that location (its first when none is given; nothing when it is not
/// stocked there). What an inventory mutation of the
/// catalog runs follows: the level's `updatedAt`, the product's derived fields and smart
/// collections (`catalog::product_changed`), the variants' `availableForSale` and
/// `variants/in_stock` or `variants/out_of_stock` when the stock crossed zero
/// (`catalog::after_inventory_change`), then `inventory_levels/update`. Returns whether a level
/// changed.
pub fn move_inventory(
    state: &mut ShopState,
    item: &str,
    location: Option<&str>,
    changes: &[(&str, i64)],
) -> bool {
    let tracked = state
        .get(item)
        .and_then(|item| item.get("tracked"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !tracked || changes.iter().all(|(_, delta)| *delta == 0) {
        return false;
    }
    let Some(level) = inventory_level(state, item, location) else {
        return false;
    };
    let product = state
        .get(item)
        .and_then(|item| item.get("variant"))
        .and_then(Value::as_str)
        .and_then(|variant| state.get(variant))
        .and_then(|variant| variant.get("product"))
        .and_then(Value::as_str)
        .map(str::to_string);
    // A bundle's quantity is its components': their bundles follow.
    let products = catalog::with_parents(state, product.into_iter().collect());
    let before = catalog::stock_of(state, &products);
    let now = state.timestamp();
    let Some(record) = state.get_mut(&level) else {
        return false;
    };
    if let Some(Value::Array(quantities)) = record.get_mut("quantities") {
        for (name, delta) in changes {
            if let Some(quantity) = quantities
                .iter_mut()
                .find(|quantity| quantity.get("name").and_then(Value::as_str) == Some(name))
            {
                let current = quantity
                    .get("quantity")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                quantity["quantity"] = json!(current + delta);
                quantity["updatedAt"] = json!(now);
            }
        }
    }
    state.touch(&level);
    catalog::after_inventory_change(state, &products, &before);
    for product in &products {
        catalog::product_changed(state, product);
    }
    state.emit("inventory_levels/update", &level);
    true
}

/// The quantity of an item at a location under a name (`committed`).
pub fn inventory_quantity(
    state: &ShopState,
    item: &str,
    location: Option<&str>,
    name: &str,
) -> i64 {
    inventory_level(state, item, location)
        .and_then(|level| state.get(&level))
        .and_then(|level| level.get("quantities"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|quantity| quantity.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|quantity| quantity.get("quantity"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// What a line item took from the stock, kept on it under this key, which GraphQL never reads:
/// `{"committed": {"<location GID>": units}, "shipped": units}`, the units it committed at each
/// location and those that shipped from them. A line of a bundle (a variant with components,
/// `requiresComponents`) takes its components' units instead, kept per component item:
/// `{"parts": {"<item GID>": {"variant", "perUnit", "committed", "shipped"}}}`. Shipping and
/// restocking a line move what it took, never what other orders committed: an order of the
/// store data or of the local checkout took nothing (they keep the stock of the data), so
/// shipping or restocking it leaves the stock as it is.
pub const LINE_STOCK: &str = "lsfStock";

/// What one unit of a variant takes from the stock: an inventory item, so many times.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StockPart {
    /// The variant whose item it is: the variant itself, or a component of a bundle.
    pub variant: String,
    pub item: String,
    /// The units of the item one unit of the variant takes (the bundle's quantity of it).
    pub per_unit: i64,
    /// Whether it is a component of a bundle (kept under `parts` in [`LINE_STOCK`]).
    pub component: bool,
}

/// What one unit of a variant takes from the stock: its own inventory item, or, for a bundle,
/// each component's item times the quantity the bundle takes of it, as shopify.dev says "the
/// bundle's inventory is determined by the inventory of its component variants" (the parent's
/// own item is not tracked).
pub fn stock_parts(state: &ShopState, variant: &str) -> Vec<StockPart> {
    let components = catalog::components_of(state, variant);
    if !components.is_empty() {
        return components
            .into_iter()
            .filter_map(|(component, per_unit)| {
                let item = text(state.get(&component)?, "inventoryItem")?.to_string();
                Some(StockPart {
                    variant: component,
                    item,
                    per_unit: per_unit.max(1),
                    component: true,
                })
            })
            .collect();
    }
    state
        .get(variant)
        .and_then(|record| text(record, "inventoryItem"))
        .map(|item| {
            vec![StockPart {
                variant: variant.to_string(),
                item: item.to_string(),
                per_unit: 1,
                component: false,
            }]
        })
        .unwrap_or_default()
}

/// The parts of `units` of a variant that a location cannot give: those whose item is tracked
/// and whose variant does not sell when out of stock (`inventoryPolicy` other than `CONTINUE`),
/// with fewer units available there than the variant takes. What `DECREMENT_OBEYING_POLICY` and
/// a billing attempt that respects the inventory policy refuse.
pub fn short_parts(
    state: &ShopState,
    variant: &str,
    location: Option<&str>,
    units: i64,
) -> Vec<StockPart> {
    stock_parts(state, variant)
        .into_iter()
        .filter(|part| {
            let tracked = state
                .get(&part.item)
                .and_then(|item| item.get("tracked"))
                .and_then(Value::as_bool)
                == Some(true);
            let continues = state
                .get(&part.variant)
                .and_then(|record| text(record, "inventoryPolicy"))
                == Some("CONTINUE");
            tracked
                && !continues
                && inventory_quantity(state, &part.item, location, "available")
                    < units.saturating_mul(part.per_unit)
        })
        .collect()
}

/// The location an order fulfills from when it names none: `orderCreate` without
/// `fulfillment.locationId`, a draft order completed, a subscription's billing. Shopify
/// documents no rule for these orders (its order routing is the checkout's): lsf takes the
/// shop's primary location (`location` without an id: the first active one) when it can give
/// every line its units, else the first active location that can, else the primary one (where
/// `DECREMENT_OBEYING_POLICY` and billing then find the stock short). A location can give a
/// line when it stocks each tracked part of it ([`stock_parts`]) and has the units of those that
/// do not sell when out of stock ([`short_parts`]). Fulfillment services' locations come last.
pub fn fulfilling_location(state: &ShopState, lines: &[(String, i64)]) -> Option<String> {
    let primary = catalog::default_location(state);
    let mut candidates: Vec<String> = primary.iter().cloned().collect();
    let mut services = Vec::new();
    for location in state.keys("Location") {
        if Some(location) == primary.as_deref() || !catalog::is_active_location(state, location) {
            continue;
        }
        let service = text_of_bool(state, location, "isFulfillmentService");
        if service {
            services.push(location.to_string());
        } else {
            candidates.push(location.to_string());
        }
    }
    candidates.extend(services);
    let gives = |location: &str| {
        lines.iter().all(|(variant, units)| {
            stock_parts(state, variant).iter().all(|part| {
                let tracked = text_of_bool(state, &part.item, "tracked");
                !tracked || inventory_level(state, &part.item, Some(location)).is_some()
            }) && short_parts(state, variant, Some(location), *units).is_empty()
        })
    };
    candidates
        .iter()
        .find(|location| gives(location))
        .cloned()
        .or(primary)
}

/// A boolean field of a record, by key (`false` when absent).
fn text_of_bool(state: &ShopState, key: &str, field: &str) -> bool {
    state
        .get(key)
        .and_then(|record| record.get(field))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The parts a line item takes from the stock: those it recorded (a bundle's components when
/// it committed them), else what its variant takes now.
fn line_parts(state: &ShopState, line: &str) -> Vec<StockPart> {
    let Some(record) = state.get(line) else {
        return Vec::new();
    };
    let recorded: Vec<StockPart> = record
        .get(LINE_STOCK)
        .and_then(|stock| stock.get("parts"))
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(item, part)| {
            Some(StockPart {
                variant: part.get("variant")?.as_str()?.to_string(),
                item: item.clone(),
                per_unit: part
                    .get("perUnit")
                    .and_then(Value::as_i64)
                    .unwrap_or(1)
                    .max(1),
                component: true,
            })
        })
        .collect();
    if !recorded.is_empty() {
        return recorded;
    }
    text(record, "variant")
        .map(|variant| stock_parts(state, variant))
        .unwrap_or_default()
}

/// The location of an item's level at a location (its first level when none is given): the key
/// a line's commitments are kept under.
fn level_location(state: &ShopState, item: &str, location: Option<&str>) -> Option<String> {
    let level = inventory_level(state, item, location)?;
    text(state.get(&level)?, "location").map(str::to_string)
}

/// What a line item recorded of a part ([`LINE_STOCK`]).
fn part_stock<'a>(state: &'a ShopState, line: &str, part: &StockPart) -> Option<&'a Value> {
    let stock = state.get(line)?.get(LINE_STOCK)?;
    if part.component {
        stock.get("parts")?.get(&part.item)
    } else {
        Some(stock)
    }
}

/// The units of a part a line item committed, by location, in the order it committed them.
fn line_commitments(state: &ShopState, line: &str, part: &StockPart) -> Vec<(String, i64)> {
    part_stock(state, line, part)
        .and_then(|stock| stock.get("committed"))
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(location, units)| Some((location.clone(), units.as_i64()?)))
        .filter(|(_, units)| *units > 0)
        .collect()
}

/// The units of a part of a line item that shipped from what it committed.
fn line_shipped(state: &ShopState, line: &str, part: &StockPart) -> i64 {
    part_stock(state, line, part)
        .and_then(|stock| stock.get("shipped"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// Adds to what a line item committed of a part at a location, and to what shipped of it.
fn record_line_stock(
    state: &mut ShopState,
    line: &str,
    part: &StockPart,
    location: &str,
    committed: i64,
    shipped: i64,
) {
    let Some(record) = state.get_mut(line) else {
        return;
    };
    let mut stock = record.entry(LINE_STOCK).or_insert_with(|| json!({}));
    if !stock.is_object() {
        *stock = json!({});
    }
    if part.component {
        if !stock.get("parts").is_some_and(Value::is_object) {
            stock["parts"] = json!({});
        }
        let parts = &mut stock["parts"];
        if !parts.get(&part.item).is_some_and(Value::is_object) {
            parts[&part.item] = json!({"variant": part.variant, "perUnit": part.per_unit});
        }
        stock = &mut parts[&part.item];
    }
    let committed = stock
        .get("committed")
        .and_then(|by_location| by_location.get(location))
        .and_then(Value::as_i64)
        .unwrap_or(0)
        + committed;
    let shipped = stock.get("shipped").and_then(Value::as_i64).unwrap_or(0) + shipped;
    if !stock.get("committed").is_some_and(Value::is_object) {
        stock["committed"] = json!({});
    }
    stock["committed"][location] = json!(committed.max(0));
    stock["shipped"] = json!(shipped.max(0));
}

/// Commits units of a part at a location (`available` to `committed`) for a line item, and
/// records them on it.
fn commit_part(
    state: &mut ShopState,
    line: &str,
    part: &StockPart,
    location: Option<&str>,
    units: i64,
) {
    let Some(at) = level_location(state, &part.item, location) else {
        return;
    };
    if units > 0
        && move_inventory(
            state,
            &part.item,
            Some(&at),
            &[("available", -units), ("committed", units)],
        )
    {
        record_line_stock(state, line, part, &at, units, 0);
    }
}

/// Commits units of a line item (`available` to `committed`), as Shopify does when an order that
/// claims inventory is created or edited, and records them on the line ([`LINE_STOCK`]): a
/// bundle's line commits its components' units ([`stock_parts`]).
pub fn commit_line(state: &mut ShopState, line: &str, location: Option<&str>, units: i64) {
    if units <= 0 {
        return;
    }
    let Some(variant) = state
        .get(line)
        .and_then(|record| text(record, "variant"))
        .map(str::to_string)
    else {
        return;
    };
    for part in stock_parts(state, &variant) {
        commit_part(
            state,
            line,
            &part,
            location,
            units.saturating_mul(part.per_unit),
        );
    }
}

/// Ships units of a line item from a location: as many as the line committed there leave
/// `committed` and `on_hand` (for a bundle, its components' units). Units it never committed
/// leave the stock as it is.
pub fn ship_line(state: &mut ShopState, line: &str, location: Option<&str>, units: i64) {
    for part in line_parts(state, line) {
        let Some(at) = level_location(state, &part.item, location) else {
            continue;
        };
        let committed = line_commitments(state, line, &part)
            .into_iter()
            .find(|(known, _)| *known == at)
            .map_or(0, |(_, units)| units);
        let units = units.saturating_mul(part.per_unit).min(committed);
        if units > 0
            && move_inventory(
                state,
                &part.item,
                Some(&at),
                &[("committed", -units), ("on_hand", -units)],
            )
        {
            record_line_stock(state, line, &part, &at, -units, units);
        }
    }
}

/// Moves what a line item committed at one location to another, as far as it committed units
/// there: a fulfillment order moved takes its stock along.
pub fn move_line_commitment(
    state: &mut ShopState,
    line: &str,
    from: Option<&str>,
    to: &str,
    units: i64,
) {
    for part in line_parts(state, line) {
        let Some(at) = level_location(state, &part.item, from) else {
            continue;
        };
        let committed = line_commitments(state, line, &part)
            .into_iter()
            .find(|(known, _)| *known == at)
            .map_or(0, |(_, units)| units);
        let units = units.saturating_mul(part.per_unit).min(committed);
        if units > 0
            && move_inventory(
                state,
                &part.item,
                Some(&at),
                &[("committed", -units), ("available", units)],
            )
        {
            record_line_stock(state, line, &part, &at, -units, 0);
            commit_part(state, line, &part, Some(to), units);
        }
    }
}

/// Puts units of a line item back in stock, as far as the line took them (for a bundle, its
/// components' units): units that had not shipped leave what it committed for `available` (at
/// the location given first, then where else it committed them); shipped units that had left
/// the stock come back to `available` and `on_hand` at the location given. Units the line never
/// took leave the stock as it is.
pub fn restock_line(
    state: &mut ShopState,
    line: &str,
    location: Option<&str>,
    units: i64,
    shipped: bool,
) {
    for part in line_parts(state, line) {
        let Some(at) = level_location(state, &part.item, location) else {
            continue;
        };
        let units = units.saturating_mul(part.per_unit);
        if shipped {
            let units = units.min(line_shipped(state, line, &part));
            if units > 0
                && move_inventory(
                    state,
                    &part.item,
                    Some(&at),
                    &[("available", units), ("on_hand", units)],
                )
            {
                record_line_stock(state, line, &part, &at, 0, -units);
            }
            continue;
        }
        let mut commitments = line_commitments(state, line, &part);
        commitments.sort_by_key(|(known, _)| *known != at);
        let mut left = units;
        for (known, committed) in commitments {
            let units = left.min(committed);
            if units <= 0 {
                break;
            }
            if move_inventory(
                state,
                &part.item,
                Some(&known),
                &[("committed", -units), ("available", units)],
            ) {
                record_line_stock(state, line, &part, &known, -units, 0);
                left -= units;
            }
        }
    }
}

/// The inventory item of a line item, through its variant.
pub fn inventory_item_of(state: &ShopState, line: &Record) -> Option<String> {
    let variant = state.get(text(line, "variant")?)?;
    text(variant, "inventoryItem").map(str::to_string)
}

/// The location a line item is fulfilled from: the assigned location of its first fulfillment
/// order.
pub fn line_location(state: &ShopState, order: &str, line: &str) -> Option<String> {
    let (fulfillment_order, _) = fulfillment_order_lines(state, order, line)
        .into_iter()
        .next()?;
    assigned_location(state, &fulfillment_order)
}

/// The location a fulfillment order is assigned to.
pub fn assigned_location(state: &ShopState, fulfillment_order: &str) -> Option<String> {
    state
        .get(fulfillment_order)?
        .get("assignedLocation")?
        .get("location")?
        .as_str()
        .map(str::to_string)
}

/// The `FulfillmentOrderAssignedLocation` of a location record, as the seed writes it.
pub fn assigned_location_value(state: &ShopState, location: &str) -> Value {
    let Some(record) = state.get(location) else {
        return json!({"name": "", "countryCode": "US", "location": null});
    };
    let address = record.get("address").cloned().unwrap_or(Value::Null);
    json!({
        "name": record.get("name"),
        "location": location,
        "address1": address.get("address1"),
        "address2": address.get("address2"),
        "city": address.get("city"),
        "province": address.get("provinceCode"),
        "zip": address.get("zip"),
        "phone": address.get("phone"),
        "countryCode": address
            .get("countryCode")
            .and_then(Value::as_str)
            .filter(|code| !code.is_empty())
            .unwrap_or("US"),
    })
}

/// The next order number of the shop: one above the highest, 1001 for the first.
pub fn next_order_number(state: &ShopState) -> u64 {
    state
        .all("Order")
        .filter_map(|order| order.get("number").and_then(Value::as_u64))
        .max()
        .unwrap_or(1000)
        .max(1000)
        + 1
}

/// Marks an order changed: `updatedAt`, and `orders/updated`.
pub fn order_updated(state: &mut ShopState, order: &str) {
    state.touch(order);
    state.emit("orders/updated", order);
}

/// A `MailingAddress` from a `MailingAddressInput`, with an id of the order's (`model_name=
/// Address`). `None` when the input is not an object.
pub fn mailing_address(input: &Value, id: &str) -> Option<Value> {
    let input = input.as_object()?;
    let get = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let country_code = get("countryCode").map(|code| code.to_ascii_uppercase());
    let country = country_code
        .as_deref()
        .and_then(lsf_core::store::reference::country_name)
        .map(str::to_string)
        .or_else(|| get("country"));
    let province_code = get("provinceCode");
    let (first, last) = (get("firstName"), get("lastName"));
    let name = match (&first, &last) {
        (Some(first), Some(last)) => Some(format!("{first} {last}")),
        (Some(name), None) | (None, Some(name)) => Some(name.clone()),
        (None, None) => None,
    };
    let area = [
        get("city"),
        province_code.clone().or_else(|| get("province")),
        get("zip"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    let mut formatted: Vec<String> = [get("company"), get("address1"), get("address2")]
        .into_iter()
        .flatten()
        .collect();
    if !area.is_empty() {
        formatted.push(area.clone());
    }
    if let Some(country) = &country {
        formatted.push(country.clone());
    }
    let formatted_area = [Some(area), country.clone()]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    Some(json!({
        "id": id,
        "firstName": first,
        "lastName": last,
        "name": name,
        "company": get("company"),
        "address1": get("address1"),
        "address2": get("address2"),
        "city": get("city"),
        "province": get("province").or_else(|| province_code.clone()),
        "provinceCode": province_code,
        "country": country,
        "countryCodeV2": country_code,
        "zip": get("zip"),
        "phone": get("phone"),
        "formatted": formatted,
        "formattedArea": if formatted_area.is_empty() { Value::Null } else { json!(formatted_area) },
    }))
}

/// `customAttributes` from an `[AttributeInput]` (`{key, value}`).
pub fn attributes(input: Option<&Value>) -> Value {
    Value::Array(
        input
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|attribute| {
                let key = attribute.get("key")?.as_str()?;
                Some(json!({"key": key, "value": attribute.get("value").cloned().unwrap_or(Value::Null)}))
            })
            .collect(),
    )
}

/// `(key, value)` of each attribute of a `customAttributes` (a `null` value is `""`), as an
/// order draft takes them.
pub fn attribute_pairs(attributes: Option<&Value>) -> Vec<(String, String)> {
    attributes
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|attribute| {
            Some((
                attribute.get("key")?.as_str()?.to_string(),
                attribute
                    .get("value")?
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ))
        })
        .collect()
}

/// An input object of a mutation argument, empty when absent.
pub fn input_of(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_amounts() {
        assert_eq!(parse_cents("19.99"), Some(1999));
        assert_eq!(parse_cents("20"), Some(2000));
        assert_eq!(parse_cents("0.125"), Some(13));
        assert_eq!(parse_cents("-5.5"), Some(-550));
        assert_eq!(parse_cents(".5"), Some(50));
        assert_eq!(parse_cents("x"), None);
        assert_eq!(parse_cents(""), None);
        // Past what 64 bits of cents hold: `None`, not an overflow.
        assert_eq!(parse_cents("92233720368547758.07"), Some(i64::MAX));
        assert_eq!(parse_cents("92233720368547758.09"), None);
        assert_eq!(parse_cents("92233720368547758.075"), None);
        assert_eq!(cents(Some(&json!({"shopMoney": {"amount": "12.5"}}))), 1250);
        assert_eq!(
            cents(Some(&json!({"amount": "3", "currencyCode": "EUR"}))),
            300
        );
        assert_eq!(cents(None), 0);
        assert_eq!(formatted_gateway("manual"), "Manual");
        assert_eq!(formatted_gateway("shopify_payments"), "Shopify Payments");
    }
}
