//! The stock moves of transfers and shipments, as the Help Center's "Creating and processing
//! inventory transfers" describes them (its table of transfer statuses):
//!
//! - **Ready to ship**: "The inventory that you're transferring is committed at the origin
//!   location" (`available` to `committed`; shopify.dev's "Manage inventory quantities and
//!   states" lists "marking transfers or shipments as ready to ship" among what moves units to
//!   `committed`).
//! - **In progress** (a shipment in transit): "Inventory is marked as incoming at the destination
//!   location" (`incoming`), and the units leave the origin (`on_hand`, from what was committed,
//!   else from `available`).
//! - **Received**: the accepted units are "made available at the destination location"
//!   (`incoming` to `available` and `on_hand`); rejected and cancelled units leave `incoming`
//!   only *(lsf's reading: they are not added to the stock)*.
//! - **Canceled**: "Your committed items are made Available at the origin location".
//!
//! Each move goes through the orders family's [`model::move_inventory`], which follows a change
//! the way an inventory mutation of the catalog does (the product's derived values, the
//! variants' `availableForSale`, `variants/in_stock` or `variants/out_of_stock`, then
//! `inventory_levels/update`). A destination that does not stock an item gets a level, as
//! Shopify activates the item there (`ACTIVATION_FAILED`: "Failed to activate inventory at
//! location."), with `inventory_levels/connect`.

use crate::domains::catalog;
use crate::domains::orders::model;
use crate::state::ShopState;

/// Commits units of an item at a location (`available` to `committed`); a negative number
/// releases them.
pub fn commit(state: &mut ShopState, item: &str, location: &str, units: i64) {
    if units != 0 {
        model::move_inventory(
            state,
            item,
            Some(location),
            &[("available", -units), ("committed", units)],
        );
    }
}

/// Ships units of an item out of a location: `committed` of them leave what was committed, the
/// rest leave `available`; all of them leave `on_hand`. Negative numbers put them back (a
/// shipment's item removed or reduced while in transit).
pub fn ship_out(state: &mut ShopState, item: &str, location: &str, committed: i64, units: i64) {
    if units != 0 {
        model::move_inventory(
            state,
            item,
            Some(location),
            &[
                ("committed", -committed),
                ("available", -(units - committed)),
                ("on_hand", -units),
            ],
        );
    }
}

/// Adds units to what is incoming at a location (a negative number takes them off), stocking
/// the item there first when it is not.
pub fn incoming(state: &mut ShopState, item: &str, location: &str, units: i64) {
    if units == 0 {
        return;
    }
    ensure_level(state, item, location);
    model::move_inventory(state, item, Some(location), &[("incoming", units)]);
}

/// Receives units at a location: they leave `incoming`; the accepted ones become `available`
/// (and `on_hand`).
pub fn receive(state: &mut ShopState, item: &str, location: &str, accepted: i64, other: i64) {
    if accepted == 0 && other == 0 {
        return;
    }
    ensure_level(state, item, location);
    model::move_inventory(
        state,
        item,
        Some(location),
        &[
            ("incoming", -(accepted + other)),
            ("available", accepted),
            ("on_hand", accepted),
        ],
    );
}

/// Whether an item is stocked at a location (has a level there).
pub fn stocked(state: &ShopState, item: &str, location: &str) -> bool {
    catalog::level_at(state, item, location).is_some()
}

/// Whether an item tracks its inventory.
pub fn tracked(state: &ShopState, item: &str) -> bool {
    state
        .get(item)
        .and_then(|record| record.get("tracked"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Stocks an item at a location that does not stock it yet: a new level, at nothing, with
/// `inventory_levels/connect`. Not at an inactive location, which stocks nothing.
fn ensure_level(state: &mut ShopState, item: &str, location: &str) {
    if !stocked(state, item, location) && catalog::is_active_location(state, location) {
        let level = catalog::new_level(state, item, location, 0);
        state.emit("inventory_levels/connect", &level);
    }
}

/// A quantity of an item at a location (0 when it is not stocked there).
pub fn quantity(state: &ShopState, item: &str, location: &str, name: &str) -> i64 {
    catalog::level_at(state, item, location)
        .and_then(|level| state.get(&level))
        .map_or(0, |level| catalog::quantity(level, name))
}
