//! Locations: `locationAdd`, `locationEdit`, `locationActivate`, `locationDeactivate`,
//! `locationDelete`, `locationLocalPickupEnable`, `locationLocalPickupDisable`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `locationAdd` creates an active location at its address (`countryCode` required, the
//!   country's name filled in), fulfilling online orders unless `fulfillsOnlineOrders: false`,
//!   with its `metafields`. A name another active location has is refused (`TAKEN`).
//!   Event: `locations/create`.
//! - `locationEdit` changes the fields given; at least one location must fulfill online orders
//!   ("At least one location must fulfill online orders.", the code's description).
//!   Event: `locations/update`.
//! - `locationDeactivate` (`@idempotent`) moves the location's inventory to
//!   `destinationLocationId` (the quantities added to its levels there, levels made where the
//!   item is not stocked yet), and refuses a location with inventory without a destination
//!   (`HAS_ACTIVE_INVENTORY_ERROR`), with open fulfillment orders
//!   (`HAS_FULFILLMENT_ORDERS_ERROR`), or the only active one
//!   (`PERMANENTLY_BLOCKED_FROM_DEACTIVATION_ERROR`). Events: `inventory_levels/update` and
//!   `inventory_levels/connect` at the destination, `inventory_levels/disconnect` for the levels
//!   moved, then `locations/deactivate`.
//! - `locationActivate` (`@idempotent`) refuses a name another active location has
//!   (`HAS_NON_UNIQUE_NAME`). Event: `locations/activate`.
//! - `locationDelete` refuses an active location ("The location cannot be deleted while it is
//!   active.", `LOCATION_IS_ACTIVE`: the reference's example) and one with inventory
//!   (`LOCATION_HAS_INVENTORY`). Event: `locations/delete`.
//! - `locationLocalPickupEnable` sets a location's `localPickupSettingsV2` (its pickup time and
//!   instructions), `locationLocalPickupDisable` removes them; an inactive location is refused
//!   (`ACTIVE_LOCATION_NOT_FOUND`), and so is the deprecated `CUSTOM` pickup time
//!   (`CUSTOM_PICKUP_TIME_NOT_ALLOWED`). shopify.dev does not say which topic these fire: lsf
//!   sends `locations/update`, as for any other change of the location.
//!
//! The store sync removes a deactivated location from the store data. Messages Shopify documents
//! only by their code are the code's description in the 2026-10 schema.

use serde_json::{Map, Value, json};

use super::inventory::with_idempotency;
use super::records::{self, coded, field, input, is_active_location, objects, text};
use super::{gids, product_changed};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("locationAdd", location_add)
        .mutation("locationEdit", location_edit)
        .mutation("locationActivate", location_activate)
        .mutation("locationDeactivate", location_deactivate)
        .mutation("locationDelete", location_delete)
        .mutation("locationLocalPickupEnable", local_pickup_enable)
        .mutation("locationLocalPickupDisable", local_pickup_disable);
}

/// A payload whose errors are in `key` (`locationDeactivateUserErrors`).
fn errors_payload(fields: Value, key: &str, errors: &[UserError]) -> Value {
    let mut payload = fields.as_object().cloned().unwrap_or_default();
    payload.insert(
        key.into(),
        Value::Array(errors.iter().map(UserError::to_json).collect()),
    );
    Value::Object(payload)
}

/// Whether an active location other than `except` fulfills online orders.
fn another_fulfills_online(state: &ShopState, except: &str) -> bool {
    state.keys("Location").any(|other| {
        other != except
            && is_active_location(state, other)
            && state
                .get(other)
                .and_then(|record| record.get("fulfillsOnlineOrders"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
    })
}

/// Whether an active location other than `except` is named `name`.
fn name_taken(state: &ShopState, name: &str, except: Option<&str>) -> bool {
    state.keys("Location").any(|location| {
        Some(location) != except
            && is_active_location(state, location)
            && state
                .get(location)
                .and_then(|record| field(record, "name"))
                .is_some_and(|known| known.eq_ignore_ascii_case(name))
    })
}

/// The `LocationAddress` of an address input, over `current`.
fn address_of(input: &Map<String, Value>, current: &Value) -> Value {
    let mut address = current.as_object().cloned().unwrap_or_default();
    for name in [
        "address1",
        "address2",
        "city",
        "phone",
        "zip",
        "provinceCode",
    ] {
        if let Some(given) = input.get(name) {
            address.insert(name.into(), given.clone());
        }
    }
    if let Some(code) = text(input, "countryCode") {
        address.insert("countryCode".into(), json!(code));
        address.insert(
            "country".into(),
            json!(lsf_core::store::reference::country_name(code).unwrap_or(code)),
        );
    }
    if input.contains_key("provinceCode") {
        // The province's name is not known to lsf: its code stands for it.
        address.insert(
            "province".into(),
            address.get("provinceCode").cloned().unwrap_or(Value::Null),
        );
    }
    for name in ["latitude", "longitude"] {
        address.entry(name).or_insert(Value::Null);
    }
    let lines: Vec<String> = [
        address.get("address1"),
        address.get("address2"),
        address.get("city"),
        address.get("zip"),
        address.get("country"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .filter(|line| !line.is_empty())
    .map(str::to_string)
    .collect();
    address.insert("formatted".into(), json!(lines));
    Value::Object(address)
}

/// The checks of a location name.
fn check_name(
    state: &ShopState,
    name: Option<&str>,
    except: Option<&str>,
    required: bool,
    errors: &mut Vec<UserError>,
) {
    match name.map(str::trim) {
        Some("") => errors.push(coded(&["input", "name"], "Name can't be blank", "BLANK")),
        None if required => errors.push(coded(&["input", "name"], "Name can't be blank", "BLANK")),
        Some(name) if name.chars().count() > 100 => errors.push(coded(
            &["input", "name"],
            "Name is too long (maximum is 100 characters)",
            "TOO_LONG",
        )),
        Some(name) if name_taken(state, name, except) => errors.push(coded(
            &["input", "name"],
            "Name has already been taken",
            "TAKEN",
        )),
        _ => {}
    }
}

/// `locationAdd`.
fn location_add(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = input(ctx, "input");
    let state = mutation.state();
    let mut errors = Vec::new();
    check_name(state, text(&input, "name"), None, true, &mut errors);
    let metafields = objects(&input, "metafields");
    errors.extend(records::check_metafields(
        state,
        None,
        "LOCATION",
        &metafields,
        &["input", "metafields"],
        Some("LocationAddUserErrorCode"),
    ));
    if !errors.is_empty() {
        return Ok(payload(json!({"location": null}), &errors));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let id = state.new_gid("Location");
    let address = address_of(
        input
            .get("address")
            .and_then(Value::as_object)
            .unwrap_or(&Map::new()),
        &json!({}),
    );
    let fulfills = input
        .get("fulfillsOnlineOrders")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    state.insert(value::object(json!({
        "__typename": "Location",
        "id": id,
        "name": text(&input, "name").unwrap_or_default().trim(),
        "address": address,
        "isActive": true,
        "fulfillsOnlineOrders": fulfills,
        "isFulfillmentService": false,
        "hasActiveInventory": false,
        "shipsInventory": true,
        "createdAt": now,
        "updatedAt": now,
        "inventoryLevels": [],
        "metafields": [],
    })));
    records::set_metafields(state, &id, "LOCATION", &metafields);
    state.emit("locations/create", &id);
    Ok(payload(json!({"location": id}), &[]))
}

/// `locationEdit`.
fn location_edit(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = input(ctx, "input");
    let state = mutation.state();
    if !state.is_of(&id, "Location") {
        let error = coded(
            &["id"],
            "The record with the ID used as the input value couldn't be found.",
            "NOT_FOUND",
        );
        return Ok(payload(json!({"location": null}), &[error]));
    }
    let mut errors = Vec::new();
    check_name(state, text(&input, "name"), Some(&id), false, &mut errors);
    if input.get("fulfillsOnlineOrders").and_then(Value::as_bool) == Some(false)
        && !another_fulfills_online(state, &id)
    {
        errors.push(coded(
            &["input", "fulfillsOnlineOrders"],
            "At least one location must fulfill online orders.",
            "CANNOT_DISABLE_ONLINE_ORDER_FULFILLMENT",
        ));
    }
    let metafields = objects(&input, "metafields");
    errors.extend(records::check_metafields(
        state,
        Some(&id),
        "LOCATION",
        &metafields,
        &["input", "metafields"],
        Some("LocationEditUserErrorCode"),
    ));
    if !errors.is_empty() {
        return Ok(payload(json!({"location": null}), &errors));
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        if let Some(name) = text(&input, "name") {
            record.insert("name".into(), json!(name.trim()));
        }
        if let Some(fulfills) = input.get("fulfillsOnlineOrders").and_then(Value::as_bool) {
            record.insert("fulfillsOnlineOrders".into(), json!(fulfills));
        }
        if let Some(address) = input.get("address").and_then(Value::as_object) {
            let current = record.get("address").cloned().unwrap_or(json!({}));
            record.insert("address".into(), address_of(address, &current));
        }
    }
    records::set_metafields(state, &id, "LOCATION", &metafields);
    state.touch(&id);
    state.emit("locations/update", &id);
    Ok(payload(json!({"location": id}), &[]))
}

/// `locationActivate`.
fn location_activate(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    with_idempotency(
        mutation,
        ctx,
        json!({"location": null}),
        "locationActivateUserErrors",
        activate_once,
    )
}

fn activate_once(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let key = "locationActivateUserErrors";
    let id = ctx.arg_str("locationId").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(record) = state.get_of(&id, "Location") else {
        let error = coded(&["locationId"], "Location not found.", "LOCATION_NOT_FOUND");
        return Ok(errors_payload(json!({"location": null}), key, &[error]));
    };
    let name = field(record, "name").unwrap_or_default().to_string();
    if is_active_location(state, &id) {
        return Ok(errors_payload(json!({"location": id}), key, &[]));
    }
    if name_taken(state, &name, Some(&id)) {
        let error = coded(
            &["locationId"],
            "There is already an active location with this name.",
            "HAS_NON_UNIQUE_NAME",
        );
        return Ok(errors_payload(json!({"location": null}), key, &[error]));
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("isActive".into(), json!(true));
        record.insert("shipsInventory".into(), json!(true));
    }
    state.touch(&id);
    state.emit("locations/activate", &id);
    Ok(errors_payload(json!({"location": id}), key, &[]))
}

/// Whether a location has inventory: levels with units on hand or coming.
fn has_inventory(state: &ShopState, location: &str) -> bool {
    state
        .get(location)
        .map(|record| gids(record.get("inventoryLevels")))
        .unwrap_or_default()
        .iter()
        .filter_map(|level| state.get(level))
        .any(|level| {
            records::quantity(level, "on_hand") != 0 || records::quantity(level, "incoming") != 0
        })
}

/// Whether open fulfillment orders are assigned to a location.
fn has_open_fulfillment_orders(state: &ShopState, location: &str) -> bool {
    state.all("FulfillmentOrder").any(|order| {
        order
            .get("assignedLocation")
            .and_then(|assigned| assigned.get("location"))
            .and_then(Value::as_str)
            == Some(location)
            && matches!(
                field(order, "status"),
                Some("OPEN" | "IN_PROGRESS" | "SCHEDULED" | "ON_HOLD")
            )
    })
}

/// `locationDeactivate`.
fn location_deactivate(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    with_idempotency(
        mutation,
        ctx,
        json!({"location": null}),
        "locationDeactivateUserErrors",
        deactivate_once,
    )
}

fn deactivate_once(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let key = "locationDeactivateUserErrors";
    let id = ctx.arg_str("locationId").unwrap_or_default().to_string();
    let destination = ctx.arg_str("destinationLocationId").map(str::to_string);
    let state = mutation.state();
    if !state.is_of(&id, "Location") {
        let error = coded(&["locationId"], "Location not found.", "LOCATION_NOT_FOUND");
        return Ok(errors_payload(json!({"location": null}), key, &[error]));
    }
    if !is_active_location(state, &id) {
        return Ok(errors_payload(json!({"location": id}), key, &[]));
    }
    let mut errors = Vec::new();
    let others_active = state
        .keys("Location")
        .any(|other| other != id && is_active_location(state, other));
    if !others_active {
        errors.push(coded(
            &["locationId"],
            "Location either has a fulfillment service or is the only location with a shipping address.",
            "PERMANENTLY_BLOCKED_FROM_DEACTIVATION_ERROR",
        ));
    }
    match &destination {
        Some(destination) if *destination == id => errors.push(coded(
            &["destinationLocationId"],
            "Destination location is the same as the location to be deactivated.",
            "DESTINATION_LOCATION_IS_THE_SAME_LOCATION",
        )),
        Some(destination)
            if !state.is_of(destination, "Location") || !is_active_location(state, destination) =>
        {
            errors.push(coded(
                &["destinationLocationId"],
                "Destination location is not found or inactive.",
                "DESTINATION_LOCATION_NOT_FOUND_OR_INACTIVE",
            ))
        }
        None if has_inventory(state, &id) => errors.push(coded(
            &["locationId"],
            "Location could not be deactivated without specifying where to relocate inventory at the location.",
            "HAS_ACTIVE_INVENTORY_ERROR",
        )),
        _ => {}
    }
    if has_open_fulfillment_orders(state, &id) {
        errors.push(coded(
            &["locationId"],
            "Location could not be deactivated because it has pending orders.",
            "HAS_FULFILLMENT_ORDERS_ERROR",
        ));
    }
    let only_online = state
        .get(&id)
        .and_then(|record| record.get("fulfillsOnlineOrders"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && !another_fulfills_online(state, &id);
    if only_online && others_active {
        errors.push(coded(
            &["locationId"],
            "At least one location must fulfill online orders.",
            "CANNOT_DISABLE_ONLINE_ORDER_FULFILLMENT",
        ));
    }
    if !errors.is_empty() {
        return Ok(errors_payload(json!({"location": null}), key, &errors));
    }

    let state = mutation.state_mut();
    let levels = state
        .get(&id)
        .map(|record| gids(record.get("inventoryLevels")))
        .unwrap_or_default();
    let products = products_of_levels(state, &levels);
    let before = records::stock_of(state, &products);
    for level in &levels {
        let Some(record) = state.get(level).cloned() else {
            continue;
        };
        let item = field(&record, "item").unwrap_or_default().to_string();
        if let Some(destination) = &destination {
            move_level(state, &record, &item, destination);
        }
        state.emit("inventory_levels/disconnect", level);
        records::remove_level(state, level);
    }
    records::after_inventory_change(state, &products, &before);
    for product in &products {
        product_changed(state, product);
    }
    // The transfers and their shipments under way follow the stock.
    if let Some(destination) = &destination {
        crate::domains::logistics::transfers::relocate(state, &id, destination);
    }
    if let Some(record) = state.get_mut(&id) {
        record.insert("isActive".into(), json!(false));
        record.insert("fulfillsOnlineOrders".into(), json!(false));
        record.insert("shipsInventory".into(), json!(false));
        record.insert("hasActiveInventory".into(), json!(false));
    }
    state.touch(&id);
    state.emit("locations/deactivate", &id);
    Ok(errors_payload(json!({"location": id}), key, &[]))
}

/// The products of the items of levels, and the bundles they are components of, as the
/// inventory mutations refresh them.
fn products_of_levels(state: &ShopState, levels: &[String]) -> Vec<String> {
    let mut products: Vec<String> = Vec::new();
    for level in levels {
        if let Some(product) = state
            .get(level)
            .and_then(|level| field(level, "item"))
            .and_then(|item| records::product_of_item(state, item))
            && !products.iter().any(|known| known == product)
        {
            products.push(product.to_string());
        }
    }
    super::with_parents(state, products)
}

/// Adds the quantities of a level to the item's level at `destination` (made when the item is
/// not stocked there).
fn move_level(state: &mut ShopState, level: &Record, item: &str, destination: &str) {
    let target = match records::level_at(state, item, destination) {
        Some(target) => target,
        None => {
            let target = records::new_level(state, item, destination, 0);
            state.emit("inventory_levels/connect", &target);
            target
        }
    };
    let now = state.timestamp();
    let quantities: Vec<(String, i64)> = level
        .get("quantities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|quantity| {
            Some((
                quantity.get("name")?.as_str()?.to_string(),
                quantity.get("quantity")?.as_i64()?,
            ))
        })
        .filter(|(_, amount)| *amount != 0)
        .collect();
    if quantities.is_empty() {
        return;
    }
    if let Some(record) = state.get_mut(&target)
        && let Some(Value::Array(entries)) = record.get_mut("quantities")
    {
        for (name, amount) in &quantities {
            if let Some(entry) = entries
                .iter_mut()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name.as_str()))
            {
                let current = entry.get("quantity").and_then(Value::as_i64).unwrap_or(0);
                entry["quantity"] = json!(current + amount);
                entry["updatedAt"] = json!(now);
            }
        }
    }
    state.touch(&target);
    state.emit("inventory_levels/update", &target);
}

/// `locationDelete`.
fn location_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let key = "locationDeleteUserErrors";
    let id = ctx.arg_str("locationId").unwrap_or_default().to_string();
    let state = mutation.state();
    let mut errors = Vec::new();
    if !state.is_of(&id, "Location") {
        errors.push(coded(
            &["locationId"],
            "Location not found.",
            "LOCATION_NOT_FOUND",
        ));
    } else if is_active_location(state, &id) {
        // shopify.dev's example.
        errors.push(coded(
            &["locationId"],
            "The location cannot be deleted while it is active.",
            "LOCATION_IS_ACTIVE",
        ));
    } else if state
        .get(&id)
        .is_some_and(|record| !gids(record.get("inventoryLevels")).is_empty())
    {
        errors.push(coded(
            &["locationId"],
            "The location cannot be deleted while it has inventory.",
            "LOCATION_HAS_INVENTORY",
        ));
    } else if has_open_fulfillment_orders(state, &id) {
        errors.push(coded(
            &["locationId"],
            "The location cannot be deleted while it has pending orders.",
            "LOCATION_HAS_PENDING_ORDERS",
        ));
    }
    if !errors.is_empty() {
        return Ok(errors_payload(
            json!({"deletedLocationId": null}),
            key,
            &errors,
        ));
    }
    let state = mutation.state_mut();
    let metafields = state
        .get(&id)
        .map(|record| gids(record.get("metafields")))
        .unwrap_or_default();
    for metafield in metafields {
        state.remove(&metafield);
    }
    state.delete(&id, "locations/delete");
    Ok(errors_payload(json!({"deletedLocationId": id}), key, &[]))
}

/// The active location of an argument, or the error of local pickup settings.
fn pickup_location(state: &ShopState, id: &str, path: &[&str]) -> Result<(), UserError> {
    if state.is_of(id, "Location") && is_active_location(state, id) {
        Ok(())
    } else {
        Err(coded(
            path,
            "Provided locationId is not for an active location belonging to this store.",
            "ACTIVE_LOCATION_NOT_FOUND",
        ))
    }
}

/// `locationLocalPickupEnable`.
fn local_pickup_enable(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let settings = input(ctx, "localPickupSettings");
    let id = text(&settings, "locationId")
        .unwrap_or_default()
        .to_string();
    let checked = pickup_location(
        mutation.state(),
        &id,
        &["localPickupSettings", "locationId"],
    )
    .and_then(|()| match text(&settings, "pickupTime") {
        Some("CUSTOM") => Err(coded(
            &["localPickupSettings", "pickupTime"],
            "Custom pickup time is not allowed for local pickup settings.",
            "CUSTOM_PICKUP_TIME_NOT_ALLOWED",
        )),
        _ => Ok(()),
    });
    if let Err(error) = checked {
        return Ok(payload(json!({"localPickupSettings": null}), &[error]));
    }
    let value = json!({
        "pickupTime": settings.get("pickupTime"),
        "instructions": text(&settings, "instructions").unwrap_or_default(),
    });
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("localPickupSettingsV2".into(), value.clone());
    }
    state.touch(&id);
    state.emit("locations/update", &id);
    Ok(payload(json!({"localPickupSettings": value}), &[]))
}

/// `locationLocalPickupDisable`.
fn local_pickup_disable(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("locationId").unwrap_or_default().to_string();
    if let Err(error) = pickup_location(mutation.state(), &id, &["locationId"]) {
        return Ok(payload(json!({"locationId": null}), &[error]));
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.remove("localPickupSettingsV2");
    }
    state.touch(&id);
    state.emit("locations/update", &id);
    Ok(payload(json!({"locationId": id}), &[]))
}
