//! (being written)

use serde_json::Value;

use super::model::{int, text};
use crate::domains::catalog::gids;
use crate::resolve::Registry;
use crate::state::ShopState;

pub fn register(_registry: &mut Registry) {}

/// What is left to return of a fulfillment line item: what shipped, less what the returns that
/// were not declined or canceled hold of it.
pub fn returnable_quantity(state: &ShopState, fulfillment_line: &str) -> i64 {
    let Some(line) = state.get(fulfillment_line) else {
        return 0;
    };
    let shipped = int(line, "quantity");
    let Some(order) = state
        .get(text(line, "lineItem").unwrap_or_default())
        .and(None::<String>)
        .or_else(|| order_of_fulfillment_line(state, fulfillment_line))
    else {
        return shipped;
    };
    let in_returns: i64 = state
        .get(&order)
        .map(|record| gids(record.get("returns")))
        .unwrap_or_default()
        .iter()
        .filter_map(|id| state.get(id))
        .filter(|record| !matches!(text(record, "status"), Some("DECLINED" | "CANCELED")))
        .flat_map(|record| gids(record.get("returnLineItems")))
        .filter_map(|id| state.get(&id))
        .filter(|item| text(item, "fulfillmentLineItem") == Some(fulfillment_line))
        .map(|item| int(item, "quantity"))
        .sum();
    (shipped - in_returns).max(0)
}

/// The order of a fulfillment line item, through the fulfillment that holds it.
fn order_of_fulfillment_line(state: &ShopState, fulfillment_line: &str) -> Option<String> {
    state.keys("Fulfillment").find_map(|fulfillment| {
        let record = state.get(fulfillment)?;
        gids(record.get("fulfillmentLineItems"))
            .iter()
            .any(|id| id == fulfillment_line)
            .then(|| text(record, "order").map(str::to_string))
            .flatten()
    })
}

#[allow(dead_code)]
fn unused(_: &Value) {}
