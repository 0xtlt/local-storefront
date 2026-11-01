//! Point of sale: the devices Shopify POS is installed on, their cash drawers, the cash
//! management reason codes, and the payment sessions a device runs (opened, counted, adjusted,
//! closed), with what they derive: the drawer's balance, the session's totals and discrepancies,
//! the summaries of a location and of the shop.
//!
//! - **The outside world**: a `PointOfSaleDevice` is a phone or a tablet running Shopify POS,
//!   which the Admin API cannot make. The seed gives each active location one device
//!   (`stable_id("point_of_sale_device", <location GID>)` *(chosen)*), so that a backend can
//!   assign it a drawer and open sessions on it, by the shop's owner (`staff::owner_gid`), as
//!   the staff member the sessions name. What Shopify would push to the device (a session
//!   opened, a drawer assigned) is sent nowhere: each is written to the shop's outbox
//!   ([`crate::outbox`], channel `pos`, `to` the device) and said in `extensions.lsf.notes`.
//! - **Reason codes**: Shopify's default codes (`CashManagementDefaultReasonCodeEnum`: cash
//!   count, payout, pickup, …) and system ones (`FLOAT_SETUP`, the automatic session events),
//!   seeded, and the custom ones `cashManagementReasonCodeCreate` makes. Only custom codes can be
//!   deleted ("SYSTEM type reason codes are not deletable").
//! - **Sales**: lsf has no POS checkout: no order is a POS sale, so a session's sales and refunds
//!   are 0, and its cash is what its counts and adjustments make. A session's activities are its
//!   counts (`CashCountActivity`: opening, mid-session, closing; the cash is the discrepancy
//!   between what was counted and what the drawer held) and adjustments (`CashAdjustmentActivity`).
//! - Shopify has no webhook topic for any of it: the mutations emit topics no subscription can
//!   name (`cash_drawers/create`, …), so the server knows the shop changed. None of it reaches
//!   the storefront or the checkout.
//!
//! The errors are the codes of each payload's error type, their descriptions the messages
//! ("A cash drawer with this name already exists at the specified location."): shopify.dev
//! documents them by code only. Each module says what shopify.dev documents of its mutations.

mod drawers;
mod sessions;
#[cfg(test)]
mod tests;

use lsf_core::util::stable_id;
use serde_json::{Value, json};

use crate::graphql::FieldContext;
use crate::resolve::{Mutation, Registry};
use crate::state::seed::Seed;
use crate::state::value::str_field as text;
use crate::state::{ShopState, gid, value};

/// The record field of a device with the location it is at.
pub const DEVICE_LOCATION: &str = "lsfLocation";

/// The default reason codes, as `CashManagementDefaultReasonCodeEnum` lists them.
const DEFAULT_CODES: &[&str] = &[
    "AUTO_END_SESSION_LOCATION_CHANGE",
    "AUTO_END_SESSION_LOGOUT",
    "AUTO_START_SESSION_CHECKOUT",
    "CASH_COUNT",
    "CASH_PAYOUT",
    "CASH_PICKUP",
    "CHANGE_CORRECTION",
    "OTHER",
    "PETTY_CASH",
    "TIP_PAYOUT",
];

/// The system reason codes, as `CashManagementSystemReasonCodeEnum` lists them (`OTHER`, which
/// 2026-10 deprecates for the default one, left out).
const SYSTEM_CODES: &[&str] = &[
    "AUTO_END_SESSION_LOCATION_CHANGE",
    "AUTO_END_SESSION_LOGOUT",
    "AUTO_START_SESSION_CHECKOUT",
    "CASH_PAYOUT",
    "FLOAT_SETUP",
];

/// The types of the reason codes, in the order `cashManagementReasonCodes` lists them.
pub const REASON_CODE_TYPES: &[&str] = &[
    "CashManagementDefaultReasonCode",
    "CashManagementSystemReasonCode",
    "CashManagementCustomReasonCode",
];

/// The GID of the device of a location.
pub fn device_gid(location: &str) -> String {
    gid(
        "PointOfSaleDevice",
        stable_id("point_of_sale_device", location),
    )
}

/// The devices and the reason codes: see the module's documentation. Runs after the other
/// families' seeds (it reads the locations).
pub fn seed(seed: &mut Seed) {
    let locations: Vec<String> = seed
        .state
        .all("Location")
        .filter(|location| location.get("isActive").and_then(Value::as_bool) == Some(true))
        .filter(|location| {
            location
                .get("isFulfillmentService")
                .and_then(Value::as_bool)
                != Some(true)
        })
        .filter_map(|location| location.get("id")?.as_str().map(str::to_string))
        .collect();
    for location in locations {
        seed.insert(json!({
            "__typename": "PointOfSaleDevice",
            "id": device_gid(&location),
            "cashDrawer": null,
            "activePaymentSession": null,
            "fiscalDeviceIdentifier": null,
            DEVICE_LOCATION: location,
        }));
    }
    for (type_name, kind, codes) in [
        (REASON_CODE_TYPES[0], "default", DEFAULT_CODES),
        (REASON_CODE_TYPES[1], "system", SYSTEM_CODES),
    ] {
        for code in codes {
            seed.insert(json!({
                "__typename": type_name,
                "id": gid(type_name, stable_id("cash_management_reason_code", &format!("{kind}/{code}"))),
                "code": code,
            }));
        }
    }
}

/// The custom resolvers and the mutations of point of sale.
pub fn register(registry: &mut Registry) {
    drawers::register(registry);
    sessions::register(registry);
}

/// A `MoneyV2` of the shop's currency.
fn money(state: &ShopState, cents: i64) -> Value {
    value::money_v2(cents, &state.currency())
}

/// The cents of a `MoneyV2` (or a `MoneyInput`).
fn cents(money: Option<&Value>) -> i64 {
    value::exact_cents(money.and_then(|money| money.get("amount"))).unwrap_or(0)
}

/// Writes what lsf would have sent a device to the shop's outbox (`what`, about the record
/// `about`), and says so in the mutation's notes.
fn tell_device(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    device: &str,
    what: &str,
    about: &str,
    message: String,
) {
    let note = format!(
        "{}: lsf sends nothing to POS devices: \"{message}\" for {device} is in the shop's \
         outbox (GET /__lsf/admin/outbox?channel=pos)",
        ctx.field_name()
    );
    crate::outbox::record(
        mutation.state_mut(),
        crate::outbox::Entry::new(crate::outbox::Channel::Pos, what)
            .to(device)
            .about(about)
            .body(message),
    );
    mutation.note(note);
}

fn is_reason_code(state: &ShopState, id: &str) -> bool {
    state
        .type_of(id)
        .is_some_and(|type_name| REASON_CODE_TYPES.contains(&type_name))
}

/// The keys of the records of `type_name` whose `field` is `value`.
fn keys_with<'s>(
    state: &'s ShopState,
    type_name: &str,
    field: &'s str,
    value: &'s str,
) -> impl Iterator<Item = &'s str> {
    state
        .keys(type_name)
        .filter(move |key| state.get(key).and_then(|record| text(record, field)) == Some(value))
}
