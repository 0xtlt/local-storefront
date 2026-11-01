//! Delivery settings of the shop: delivery promise providers and participants
//! (`deliveryPromiseProviderUpsert`, `deliveryPromiseParticipantsUpdate`,
//! `deliveryPromiseProvider`, `deliveryPromiseParticipants`), `deliveryPromiseSettings`,
//! `deliverySettingUpdate`, `deliveryShippingOriginAssign`, and the shop's shipping packages
//! (`shippingPackageUpdate`, `shippingPackageMakeDefault`, `shippingPackageDelete`).
//!
//! - A **delivery promise provider** is a location's ("The ID of the location that will be
//!   associated with the delivery promise provider"), "currently restricted to select approved
//!   delivery promise partners": the location must be one of the app's fulfillment service
//!   (`MUST_BELONG_TO_APP`). It is created `active`, in `UTC` ("Defaults to `UTC` when creating a
//!   provider"), and updated in place; a time zone must be an IANA one (`INVALID_TIME_ZONE`).
//! - **Participants** of a branded promise are product variants, added and removed by handle;
//!   the payload answers those added. The messages are lsf's (the payload's `UserError` has no
//!   code).
//! - **`deliveryPromiseSettings`** is the shop's `DeliveryPromiseSetting` (the seed's: delivery
//!   dates off, no processing time; the API has no mutation for it, the admin does:
//!   `delivery_promise_settings/update` is built for a trigger).
//! - **`deliverySettingUpdate`** takes no input in 2026-10: it changes nothing and succeeds.
//!   **`deliveryShippingOriginAssign`** is deprecated as of 2026-04, "as single origin shipping
//!   mode has been retired": no shop is in that mode, so it answers that, and changes nothing.
//! - **Shipping packages** have no GraphQL type: Shopify's admin makes them, the API only
//!   updates, picks the default ("the one used to calculate shipping costs on checkout") or
//!   deletes them. Every shop has the one Shopify adds, a "Sample box" *(chosen: its size and
//!   weight are lsf's)*, under the id `gid://shopify/ShippingPackage/1`. They are kept with the
//!   shop (`GET /__lsf/admin` shows them, `shippingPackages`); a label bought without a package
//!   uses the default. Their messages are lsf's.

use chrono_tz::Tz;
use serde_json::{Value, json};

use super::text;
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::seed::Seed;
use crate::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("deliveryPromiseProviderUpsert", provider_upsert)
        .mutation("deliveryPromiseParticipantsUpdate", participants_update)
        .mutation("deliverySettingUpdate", |_m, _ctx| {
            Ok(payload(json!({}), &[]))
        })
        .mutation("deliveryShippingOriginAssign", shipping_origin_assign)
        .mutation("shippingPackageUpdate", package_update)
        .mutation("shippingPackageMakeDefault", package_make_default)
        .mutation("shippingPackageDelete", package_delete)
        .query("deliveryPromiseProvider", provider)
        .query("deliveryPromiseParticipants", participants);
}

/// The key of the shop's `DeliveryPromiseSetting`, which has no id.
pub const PROMISE_SETTING: &str = "DeliveryPromiseSetting";

/// The memo the shop's shipping packages are kept under.
const PACKAGES: &str = "logistics/shipping-packages";

/// The shop's delivery promise settings and its sample package.
pub fn seed(seed: &mut Seed) {
    seed.state.insert_as(
        PROMISE_SETTING,
        value::object(json!({
            "__typename": "DeliveryPromiseSetting",
            "deliveryDatesEnabled": false,
            "processingTime": null,
        })),
    );
    seed.state.set_memo(
        PACKAGES,
        json!([{
            "id": "gid://shopify/ShippingPackage/1",
            "name": "Sample box",
            "type": "BOX",
            "default": true,
            "dimensions": {"length": 22.0, "width": 13.7, "height": 4.2, "unit": "CENTIMETERS"},
            "weight": {"value": 0.0, "unit": "GRAMS"},
        }]),
    );
}

/// The shop's shipping packages, as `GET /__lsf/admin` shows them.
pub fn packages(state: &ShopState) -> Vec<Value> {
    state
        .memo(PACKAGES)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The default shipping package, when there is one.
pub fn default_package(state: &ShopState) -> Option<Value> {
    packages(state)
        .into_iter()
        .find(|package| package.get("default").and_then(Value::as_bool) == Some(true))
}

fn set_packages(state: &mut ShopState, packages: Vec<Value>) {
    state.set_memo(PACKAGES, Value::Array(packages));
}

// --- Delivery promises ---

fn provider_upsert(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"deliveryPromiseProvider": null});
    let location = ctx.arg_str("locationId").unwrap_or_default().to_string();
    let state = mutation.state();
    let fail =
        |field: &str, code: &str| Ok(payload(empty.clone(), &[super::coded(ctx, &[field], code)]));
    let Some(record) = state.get_of(&location, "Location") else {
        return fail("locationId", "NOT_FOUND");
    };
    if record.get("isFulfillmentService").and_then(Value::as_bool) != Some(true) {
        return fail("locationId", "MUST_BELONG_TO_APP");
    }
    if let Some(zone) = ctx.arg_str("timeZone") {
        if zone.chars().count() > 255 {
            return fail("timeZone", "TOO_LONG");
        }
        if zone.parse::<Tz>().is_err() {
            return fail("timeZone", "INVALID_TIME_ZONE");
        }
    }
    let existing = state
        .all("DeliveryPromiseProvider")
        .find(|provider| text(provider, "location") == Some(location.as_str()))
        .and_then(|provider| text(provider, "id").map(str::to_string));
    let state = mutation.state_mut();
    let id = match existing {
        Some(id) => id,
        None => {
            let id = state.new_gid("DeliveryPromiseProvider");
            state.insert(value::object(json!({
                "__typename": "DeliveryPromiseProvider",
                "id": id,
                "location": location,
                "active": true,
                "fulfillmentDelay": null,
                "timeZone": "UTC",
            })));
            id
        }
    };
    if let Some(record) = state.get_mut(&id) {
        for field in ["active", "fulfillmentDelay", "timeZone"] {
            if let Some(given) = ctx.arg(field).filter(|given| !given.is_null()) {
                record.insert(field.into(), given.clone());
            }
        }
    }
    state.emit(crate::domains::app::CHANGED, &id);
    Ok(payload(json!({"deliveryPromiseProvider": id}), &[]))
}

/// `deliveryPromiseProvider(locationId:)`: the provider of a location, `null` without one.
fn provider(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let location = ctx.arg_str("locationId").unwrap_or_default();
    Ok(r.state()
        .all("DeliveryPromiseProvider")
        .find(|provider| text(provider, "location") == Some(location))
        .and_then(|provider| text(provider, "id"))
        .map_or(Output::Null, |id| Output::Object(Obj::record(id))))
}

/// The record field where a participant keeps its branded promise's handle.
const HANDLE: &str = "lsfBrandedPromiseHandle";

fn participants_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"promiseParticipants": null});
    let handle = ctx
        .arg_str("brandedPromiseHandle")
        .unwrap_or_default()
        .trim()
        .to_string();
    let adding = gids(ctx.arg("ownersToAdd"));
    let removing = gids(ctx.arg("ownersToRemove"));
    let state = mutation.state();
    let mut errors = Vec::new();
    if handle.is_empty() {
        errors.push(UserError::new(
            &["brandedPromiseHandle"],
            "Branded promise handle can't be blank",
        ));
    }
    for (field, owners) in [("ownersToAdd", &adding), ("ownersToRemove", &removing)] {
        for (index, owner) in owners.iter().enumerate() {
            if state.type_of(owner) != Some("ProductVariant") {
                errors.push(UserError::new(
                    &[field, &index.to_string()],
                    format!("Owner {owner} is not a product variant of the shop"),
                ));
            }
        }
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let of_handle: Vec<(String, String)> = state
        .all("DeliveryPromiseParticipant")
        .filter(|participant| text(participant, HANDLE) == Some(handle.as_str()))
        .filter_map(|participant| {
            Some((
                text(participant, "id")?.to_string(),
                text(participant, "owner")?.to_string(),
            ))
        })
        .collect();
    let state = mutation.state_mut();
    for (id, owner) in &of_handle {
        if removing.contains(owner) {
            state.emit(crate::domains::app::CHANGED, id);
            state.remove(id);
        }
    }
    let mut added = Vec::new();
    for owner in &adding {
        if of_handle.iter().any(|(_, known)| known == owner) || removing.contains(owner) {
            continue;
        }
        let id = state.new_gid("DeliveryPromiseParticipant");
        state.insert(value::object(json!({
            "__typename": "DeliveryPromiseParticipant",
            "id": id,
            "owner": owner,
            "ownerType": "PRODUCTVARIANT",
            HANDLE: handle,
        })));
        state.emit(crate::domains::app::CHANGED, &id);
        added.push(json!(id));
    }
    Ok(payload(json!({"promiseParticipants": added}), &[]))
}

/// `deliveryPromiseParticipants(brandedPromiseHandle:, ownerIds:)`.
fn participants(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let handle = ctx.arg_str("brandedPromiseHandle").unwrap_or_default();
    let owners = ctx.arg("ownerIds").map(|ids| gids(Some(ids)));
    let items = r
        .state()
        .all("DeliveryPromiseParticipant")
        .filter(|participant| text(participant, HANDLE) == Some(handle))
        .filter(|participant| {
            owners.as_ref().is_none_or(|owners| {
                text(participant, "owner").is_some_and(|owner| owners.iter().any(|id| id == owner))
            })
        })
        .filter_map(|participant| participant.get("id").cloned())
        .collect();
    r.connection(ctx, items)
}

// --- Shipping origin ---

/// `deliveryShippingOriginAssign`: refused, single origin shipping being retired.
fn shipping_origin_assign(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let location = ctx.arg_str("locationId").unwrap_or_default();
    // lsf's wording, from the deprecation reason of the schema.
    let message = if mutation.state().type_of(location) == Some("Location") {
        "Single origin shipping mode is no longer supported."
    } else {
        "Location not found."
    };
    Ok(payload(
        json!({}),
        &[UserError::new(&["locationId"], message)],
    ))
}

// --- Shipping packages ---

/// The position of a package among the shop's, or the error of an id that is none.
fn package_index(state: &ShopState, id: &str) -> Result<usize, UserError> {
    packages(state)
        .iter()
        .position(|package| package.get("id").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| UserError::new(&["id"], "Shipping package not found."))
}

fn package_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let index = match package_index(mutation.state(), id) {
        Ok(index) => index,
        Err(error) => return Ok(payload(json!({}), &[error])),
    };
    let input = ctx.arg_object_or_default("shippingPackage");
    let mut errors = Vec::new();
    if let Some(name) = input.get("name").and_then(Value::as_str)
        && name.trim().is_empty()
    {
        errors.push(UserError::new(
            &["shippingPackage", "name"],
            "Name can't be blank",
        ));
    }
    if let Some(dimensions) = input.get("dimensions").filter(|value| value.is_object())
        && ["length", "width", "height"]
            .iter()
            .any(|side| dimensions.get(side).and_then(Value::as_f64).unwrap_or(0.0) <= 0.0)
    {
        errors.push(UserError::new(
            &["shippingPackage", "dimensions"],
            "Dimensions must be greater than 0",
        ));
    }
    if input
        .get("weight")
        .and_then(|weight| weight.get("value"))
        .and_then(Value::as_f64)
        .is_some_and(|weight| weight < 0.0)
    {
        errors.push(UserError::new(
            &["shippingPackage", "weight"],
            "Weight must be greater than or equal to 0",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({}), &errors));
    }
    let state = mutation.state_mut();
    let mut all = packages(state);
    let make_default = input.get("default").and_then(Value::as_bool) == Some(true);
    if make_default {
        for package in &mut all {
            package["default"] = json!(false);
        }
    }
    if let Some(Value::Object(package)) = all.get_mut(index) {
        for field in ["name", "type", "dimensions", "weight"] {
            if let Some(given) = input.get(field).filter(|given| !given.is_null()) {
                package.insert(field.into(), given.clone());
            }
        }
        if make_default {
            package.insert("default".into(), json!(true));
        }
    }
    set_packages(state, all);
    state.emit(crate::domains::app::CHANGED, id);
    Ok(payload(json!({}), &[]))
}

fn package_make_default(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let index = match package_index(mutation.state(), id) {
        Ok(index) => index,
        Err(error) => return Ok(payload(json!({}), &[error])),
    };
    let state = mutation.state_mut();
    let mut all = packages(state);
    for (position, package) in all.iter_mut().enumerate() {
        package["default"] = json!(position == index);
    }
    set_packages(state, all);
    state.emit(crate::domains::app::CHANGED, id);
    Ok(payload(json!({}), &[]))
}

fn package_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let index = match package_index(mutation.state(), &id) {
        Ok(index) => index,
        Err(error) => return Ok(payload(json!({"deletedId": null}), &[error])),
    };
    let state = mutation.state_mut();
    let mut all = packages(state);
    all.remove(index);
    set_packages(state, all);
    state.emit(crate::domains::app::CHANGED, &id);
    Ok(payload(json!({"deletedId": id}), &[]))
}
