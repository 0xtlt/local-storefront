//! The rest of order edits: shipping lines (`orderEditAddShippingLine`,
//! `orderEditUpdateShippingLine`, `orderEditRemoveShippingLine`), a discount changed
//! (`orderEditUpdateDiscount`), and `orderEditRemoveLineItemDiscount`, the deprecated form of
//! `orderEditRemoveDiscount`.
//!
//! As shopify.dev's reference of 2026-10 describes them, with its examples:
//!
//! - The calculated order lists its shipping lines (`CalculatedShippingLine`): the order's, staged
//!   `NONE`, and those the edit adds, `ADDED`, whose id is a `CalculatedShippingLine` UUID (as in
//!   the examples). Only an added one can be updated ("Only staged shipping lines can be
//!   updated, whereas committed shipping lines may only be removed"); removing an added one drops
//!   it, removing one of the order's stages it `REMOVED`. The totals and the outstanding amount
//!   follow (`totalPriceSet` 31.49 for 11.50 of items and a 19.99 shipping line added).
//! - `orderEditUpdateDiscount` changes a discount added in the edit (its value, its description);
//!   `orderEditRemoveLineItemDiscount` removes one and answers its line item.
//! - The commit adds the shipping lines added (custom lines, at their price) and marks those
//!   removed `isRemoved`; the order's shipping is what is left. Nothing fires before the commit.
//!
//! Messages: lsf's wording, with the payloads' `INVALID`; shopify.dev shows none.

use serde_json::{Map, Value, json};

use super::edits::{
    calculated_line, calculated_order, change_line, discount_of, refresh_calculated,
    remove_discount, staged,
};
use super::model;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("orderEditAddShippingLine", add_shipping_line)
        .mutation("orderEditRemoveLineItemDiscount", remove_line_item_discount)
        .mutation("orderEditRemoveShippingLine", remove_shipping_line)
        .mutation("orderEditUpdateDiscount", update_discount)
        .mutation("orderEditUpdateShippingLine", update_shipping_line);
}

fn invalid(field: &[&str], message: &str) -> UserError {
    UserError::new(field, message).code("INVALID")
}

/// The calculated shipping lines of a calculated order.
fn shipping_lines(state: &ShopState, calculated: &str) -> Vec<Value> {
    state
        .get(calculated)
        .map(|record| model::array(record, "shippingLines"))
        .unwrap_or_default()
}

fn set_shipping_lines(state: &mut ShopState, calculated: &str, lines: Vec<Value>) {
    if let Some(record) = state.get_mut(calculated) {
        record.insert("shippingLines".into(), Value::Array(lines));
    }
    refresh_calculated(state, calculated);
}

/// A shipping line's price and title from an input (`OrderEditAddShippingLineInput`,
/// `OrderEditUpdateShippingLineInput`): what it gives, checked.
fn read_shipping(
    state: &ShopState,
    input: &Map<String, Value>,
    required: bool,
) -> Result<(Option<String>, Option<i64>), UserError> {
    let title = match input.get("title").filter(|title| !title.is_null()) {
        Some(title) => {
            let title = title.as_str().unwrap_or_default().trim().to_string();
            if title.is_empty() {
                return Err(invalid(&["shippingLine", "title"], "Title can't be blank"));
            }
            Some(title)
        }
        None if required => {
            return Err(invalid(&["shippingLine", "title"], "Title can't be blank"));
        }
        None => None,
    };
    let price = match input.get("price").filter(|price| !price.is_null()) {
        Some(price) => {
            if let Some(currency) = price.get("currencyCode").and_then(Value::as_str)
                && currency != model::currency(state)
            {
                return Err(invalid(
                    &["shippingLine", "price", "currencyCode"],
                    "The currency must be the order's currency",
                ));
            }
            let cents = model::cents(Some(price));
            if cents < 0 {
                return Err(invalid(
                    &["shippingLine", "price", "amount"],
                    "Price can't be negative",
                ));
            }
            Some(cents)
        }
        None => None,
    };
    Ok((title, price))
}

/// `orderEditAddShippingLine`.
fn add_shipping_line(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| {
        Ok(payload(
            json!({"calculatedOrder": null, "calculatedShippingLine": null, "orderEditSession": null}),
            &[error],
        ))
    };
    let state = mutation.state();
    let calculated = match calculated_order(state, ctx.arg_str("id").unwrap_or_default()) {
        Ok(calculated) => calculated,
        Err(error) => return failed(error.code("INVALID")),
    };
    let input = model::input_of(ctx.arg("shippingLine"));
    let (title, price) = match read_shipping(state, &input, true) {
        Ok(shipping) => shipping,
        Err(error) => return failed(error),
    };
    let currency = model::currency(state);
    let mut lines = shipping_lines(state, &calculated);
    let id = format!(
        "gid://shopify/CalculatedShippingLine/{}",
        value::uuid(&format!("{}/{calculated}/{}", state.domain(), lines.len()))
    );
    let line = json!({
        "id": id,
        "title": title,
        "price": value::money_bag(price.unwrap_or(0), &currency),
        "stagedStatus": "ADDED",
    });
    lines.push(line.clone());
    let state = mutation.state_mut();
    set_shipping_lines(state, &calculated, lines);
    Ok(staged(
        state,
        &calculated,
        None,
        json!({"calculatedShippingLine": line}),
    ))
}

/// The calculated order of `id`, its shipping lines, and the position among them of
/// `shippingLineId`.
fn shipping_line_of(
    state: &ShopState,
    ctx: &FieldContext<'_>,
) -> Result<(String, Vec<Value>, usize), UserError> {
    let calculated = calculated_order(state, ctx.arg_str("id").unwrap_or_default())
        .map_err(|error| error.code("INVALID"))?;
    let id = ctx.arg_str("shippingLineId").unwrap_or_default();
    let lines = shipping_lines(state, &calculated);
    let position = lines
        .iter()
        .position(|line| line.get("id").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| invalid(&["shippingLineId"], "Shipping line does not exist"))?;
    Ok((calculated, lines, position))
}

/// `orderEditUpdateShippingLine`: a shipping line added in the edit.
fn update_shipping_line(
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
    let (calculated, mut lines, position) = match shipping_line_of(state, ctx) {
        Ok(found) => found,
        Err(error) => return failed(error),
    };
    if lines[position].get("stagedStatus").and_then(Value::as_str) != Some("ADDED") {
        return failed(invalid(
            &["shippingLineId"],
            "Only a shipping line added in this edit can be updated: the order's may only be removed",
        ));
    }
    let input = model::input_of(ctx.arg("shippingLine"));
    let (title, price) = match read_shipping(state, &input, false) {
        Ok(shipping) => shipping,
        Err(error) => return failed(error),
    };
    let currency = model::currency(state);
    if let Some(title) = title {
        lines[position]["title"] = json!(title);
    }
    if let Some(price) = price {
        lines[position]["price"] = value::money_bag(price, &currency);
    }
    let state = mutation.state_mut();
    set_shipping_lines(state, &calculated, lines);
    Ok(staged(state, &calculated, None, json!({})))
}

/// `orderEditRemoveShippingLine`: drops a line added in the edit, stages one of the order's
/// `REMOVED`.
fn remove_shipping_line(
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
    let (calculated, mut lines, position) = match shipping_line_of(state, ctx) {
        Ok(found) => found,
        Err(error) => return failed(error),
    };
    match lines[position].get("stagedStatus").and_then(Value::as_str) {
        Some("ADDED") => {
            lines.remove(position);
        }
        Some("REMOVED") => {
            return failed(invalid(
                &["shippingLineId"],
                "The shipping line is already removed",
            ));
        }
        _ => lines[position]["stagedStatus"] = json!("REMOVED"),
    }
    let state = mutation.state_mut();
    set_shipping_lines(state, &calculated, lines);
    Ok(staged(state, &calculated, None, json!({})))
}

/// The calculated line item that holds a discount added in the edit, by the discount
/// application's id.
fn line_of_discount(state: &ShopState, calculated: &str, application: &str) -> Option<Value> {
    state
        .get(calculated)?
        .get("lineItems")?
        .as_array()?
        .iter()
        .find(|line| {
            line.pointer("/calculatedDiscountAllocations/0/discountApplication/id")
                .and_then(Value::as_str)
                == Some(application)
        })
        .cloned()
}

/// `orderEditUpdateDiscount`: a discount added in the edit, with a new value or description.
fn update_discount(
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
    let Some(line) = line_of_discount(state, &calculated, &application) else {
        return failed(invalid(
            &["discountApplicationId"],
            "The discount does not exist on this edit",
        ));
    };
    let discount = model::input_of(ctx.arg("discount"));
    let currency = model::currency(state);
    let (amount, value) = match discount_of(&line, &discount, &currency) {
        Ok(discount) => discount,
        Err(error) => return failed(error.code("INVALID")),
    };
    let line_id = line
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let description = discount.get("description").cloned();
    let mut applied = line
        .pointer("/calculatedDiscountAllocations/0/discountApplication")
        .cloned()
        .unwrap_or(Value::Null);
    applied["value"] = value;
    if let Some(description) = description {
        applied["description"] = description;
    }
    let allocation = json!({
        "allocatedAmountSet": value::money_bag(amount, &currency),
        "discountApplication": applied,
    });
    let state = mutation.state_mut();
    change_line(state, &calculated, &line_id, |line| {
        line.insert("lsfDiscount".into(), json!(amount));
        line.insert("calculatedDiscountAllocations".into(), json!([allocation]));
    });
    if let Some(Value::Array(applications)) = state
        .get_mut(&calculated)
        .and_then(|record| record.get_mut("addedDiscountApplications"))
    {
        for known in applications.iter_mut() {
            if known.get("id").and_then(Value::as_str) == Some(application.as_str()) {
                *known = applied.clone();
            }
        }
    }
    Ok(staged(state, &calculated, None, json!({})))
}

/// `orderEditRemoveLineItemDiscount`: `orderEditRemoveDiscount`, answering the line item.
fn remove_line_item_discount(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let calculated = calculated_order(mutation.state(), ctx.arg_str("id").unwrap_or_default());
    let line = calculated.as_ref().ok().and_then(|calculated| {
        line_of_discount(
            mutation.state(),
            calculated,
            ctx.arg_str("discountApplicationId").unwrap_or_default(),
        )
    });
    let mut answer = remove_discount(mutation, ctx)?;
    let removed = answer
        .get("userErrors")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty);
    answer["calculatedLineItem"] = match (removed, calculated, line) {
        (true, Ok(calculated), Some(line)) => line
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| calculated_line(mutation.state(), &calculated, id))
            .unwrap_or(Value::Null),
        _ => Value::Null,
    };
    Ok(answer)
}

/// Applies the shipping lines of a committed edit to its order: those added become custom
/// shipping lines, those removed are `isRemoved`; the order's shipping is what is left.
pub(super) fn commit_shipping(state: &mut ShopState, order: &str, calculated: &Record) {
    let lines = model::array(calculated, "shippingLines");
    if lines
        .iter()
        .all(|line| line.get("stagedStatus").and_then(Value::as_str) == Some("NONE"))
    {
        return;
    }
    let currency = model::currency(state);
    let now = state.timestamp();
    let Some(record) = state.get(order).cloned() else {
        return;
    };
    let mut shipping = model::array(&record, "shippingLines");
    for line in &lines {
        match line.get("stagedStatus").and_then(Value::as_str) {
            Some("ADDED") => {
                let price = model::cents(line.get("price"));
                let id = state.new_gid("ShippingLine");
                shipping.push(json!({
                    "id": id,
                    "title": line.get("title"),
                    "code": line.get("title"),
                    "source": "shopify",
                    "custom": true,
                    "isRemoved": false,
                    "carrierIdentifier": null,
                    "shippingRateHandle": null,
                    "phone": null,
                    "originalPriceSet": value::money_bag(price, &currency),
                    "discountedPriceSet": value::money_bag(price, &currency),
                    "currentDiscountedPriceSet": value::money_bag(price, &currency),
                    "discountAllocations": [],
                    "taxLines": [],
                    "createdAt": now,
                }));
            }
            Some("REMOVED") => {
                for known in shipping.iter_mut() {
                    if known.get("id") == line.get("id") {
                        known["isRemoved"] = json!(true);
                    }
                }
            }
            _ => {}
        }
    }
    let kept: Vec<&Value> = shipping
        .iter()
        .filter(|line| line.get("isRemoved").and_then(Value::as_bool) != Some(true))
        .collect();
    let total: i64 = kept
        .iter()
        .map(|line| model::cents(line.get("discountedPriceSet")))
        .sum();
    let first = kept
        .first()
        .map(|line| (*line).clone())
        .unwrap_or(Value::Null);
    if let Some(record) = state.get_mut(order) {
        record.insert(
            "totalShippingPriceSet".into(),
            value::money_bag(total, &currency),
        );
        record.insert("shippingLine".into(), first);
        record.insert("shippingLines".into(), Value::Array(shipping));
    }
}
