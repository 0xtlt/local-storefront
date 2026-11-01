//! Cash drawers, the devices assigned to them, and the reason codes: `cashDrawerCreate`,
//! `cashDrawerFindOrCreate`, `cashDrawerUpdate`, `pointOfSaleDeviceAssignToCashDrawer`,
//! `cashManagementReasonCodeCreate`, `cashManagementReasonCodeDelete`, and
//! `cashManagementReasonCodes`, `CashDrawer.pointOfSaleDevices`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `cashDrawerCreate(locationId:, name:)`: a drawer at a location; its name is unique there
//!   (`CASH_DRAWER_ALREADY_EXISTS`). Its `balance` starts at 0, in the shop's currency.
//! - `cashDrawerFindOrCreate(locationId:, name:, pointOfSaleDeviceId:)` "Finds or creates a cash
//!   drawer for cash management. Also ensures the provided device is assigned to the drawer."
//! - `cashDrawerUpdate(id:, input: {name})` renames it.
//! - `pointOfSaleDeviceAssignToCashDrawer` "Assigns a point of sale device to a cash drawer,
//!   removing any prior assignment"; "The cash drawer and point of sale device must be in the
//!   same location" (`LOCATION_MISMATCH`).
//! - `cashManagementReasonCodeCreate(code:)` makes a custom code (`CODE_ALREADY_EXISTS` when a
//!   code of any kind has it); `cashManagementReasonCodeDelete(id:)` deletes a custom one
//!   ("SYSTEM type reason codes are not deletable", `NOT_DELETABLE`, as the defaults).

use serde_json::{Value, json};

use super::{
    DEVICE_LOCATION, REASON_CODE_TYPES, is_reason_code, keys_with, money, tell_device, text,
};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("cashDrawerCreate", drawer_create)
        .mutation("cashDrawerFindOrCreate", drawer_find_or_create)
        .mutation("cashDrawerUpdate", drawer_update)
        .mutation("pointOfSaleDeviceAssignToCashDrawer", assign)
        .mutation("cashManagementReasonCodeCreate", reason_code_create)
        .mutation("cashManagementReasonCodeDelete", reason_code_delete)
        .query("cashManagementReasonCodes", reason_codes)
        .field("CashDrawer", "pointOfSaleDevices", drawer_devices);
}

/// The longest drawer name lsf takes (Shopify's limit is not published).
const NAME_LENGTH: usize = 255;

/// A user error with its code; the message is the code's description.
fn coded(field: &[&str], code: &str) -> UserError {
    let message = match code {
        "CASH_DRAWER_ALREADY_EXISTS" => {
            "A cash drawer with this name already exists at the specified location."
        }
        "CASH_DRAWER_NOT_FOUND" => "The cash drawer was not found.",
        "INVALID_NAME" => "The name provided is invalid.",
        "LOCATION_NOT_FOUND" => "The location was not found.",
        "POINT_OF_SALE_DEVICE_NOT_FOUND" => "The point of sale device was not found.",
        "LOCATION_MISMATCH" => {
            "The cash drawer and point of sale device must be in the same location."
        }
        "BLANK" => "The input value is blank.",
        "CODE_ALREADY_EXISTS" => "The reason code already exists.",
        "NOT_DELETABLE" => "SYSTEM type reason codes are not deletable.",
        "NOT_FOUND" => "Reason code not found.",
        _ => "Unexpected internal error happened.",
    };
    UserError::new(field, message).code(code)
}

/// The drawer of a location with a name (ignoring case).
fn drawer_named(
    state: &ShopState,
    location: &str,
    name: &str,
    except: Option<&str>,
) -> Option<String> {
    state
        .all("CashDrawer")
        .find(|drawer| {
            text(drawer, "location") == Some(location)
                && text(drawer, "name").is_some_and(|known| known.eq_ignore_ascii_case(name))
                && text(drawer, "id") != except
        })
        .and_then(|drawer| text(drawer, "id").map(str::to_string))
}

fn name_ok(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= NAME_LENGTH
}

fn new_drawer(state: &mut ShopState, location: &str, name: &str) -> String {
    let id = state.new_gid("CashDrawer");
    let balance = money(state, 0);
    state.insert(value::object(json!({
        "__typename": "CashDrawer",
        "id": id,
        "name": name.trim(),
        "location": location,
        "balance": balance,
    })));
    state.emit("cash_drawers/create", &id);
    id
}

/// Assigns a device to a drawer (its prior assignment goes) and tells the device, unless it
/// already was.
fn assign_device(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>, device: &str, drawer: &str) {
    let state = mutation.state();
    if state
        .get(device)
        .and_then(|record| text(record, "cashDrawer"))
        == Some(drawer)
    {
        return;
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(device) {
        record.insert("cashDrawer".into(), json!(drawer));
    }
    state.emit("point_of_sale_devices/update", device);
    tell_device(
        mutation,
        ctx,
        device,
        "cash_drawer_assigned",
        drawer,
        format!("assigned to the cash drawer {drawer}"),
    );
}

fn drawer_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"cashDrawer": null});
    let state = mutation.state();
    let location = ctx.arg_str("locationId").unwrap_or_default().to_string();
    let name = ctx.arg_str("name").unwrap_or_default().to_string();
    if state.type_of(&location) != Some("Location") {
        return Ok(payload(
            empty,
            &[coded(&["locationId"], "LOCATION_NOT_FOUND")],
        ));
    }
    if !name_ok(&name) {
        return Ok(payload(empty, &[coded(&["name"], "INVALID_NAME")]));
    }
    if drawer_named(state, &location, &name, None).is_some() {
        return Ok(payload(
            empty,
            &[coded(&["name"], "CASH_DRAWER_ALREADY_EXISTS")],
        ));
    }
    let drawer = new_drawer(mutation.state_mut(), &location, &name);
    Ok(payload(json!({"cashDrawer": drawer}), &[]))
}

fn drawer_find_or_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"cashDrawer": null});
    let state = mutation.state();
    let location = ctx.arg_str("locationId").unwrap_or_default().to_string();
    let name = ctx.arg_str("name").unwrap_or_default().to_string();
    let device = ctx
        .arg_str("pointOfSaleDeviceId")
        .unwrap_or_default()
        .to_string();
    if state.type_of(&location) != Some("Location") {
        return Ok(payload(
            empty,
            &[coded(&["locationId"], "LOCATION_NOT_FOUND")],
        ));
    }
    if state.type_of(&device) != Some("PointOfSaleDevice") {
        return Ok(payload(
            empty,
            &[coded(
                &["pointOfSaleDeviceId"],
                "POINT_OF_SALE_DEVICE_NOT_FOUND",
            )],
        ));
    }
    if !name_ok(&name) {
        // `CashDrawerFindOrCreateUserErrorCode` has no `INVALID_NAME`: `NOT_SAVED`, "Failed to
        // find or create cash drawer.".
        let error =
            UserError::new(&["name"], "Failed to find or create cash drawer.").code("NOT_SAVED");
        return Ok(payload(empty, &[error]));
    }
    let found = drawer_named(state, &location, &name, None);
    let drawer = found.unwrap_or_else(|| new_drawer(mutation.state_mut(), &location, &name));
    assign_device(mutation, ctx, &device, &drawer);
    Ok(payload(json!({"cashDrawer": drawer}), &[]))
}

fn drawer_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"cashDrawer": null});
    let state = mutation.state();
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let Some(record) = state.get_of(&id, "CashDrawer") else {
        return Ok(payload(empty, &[coded(&["id"], "CASH_DRAWER_NOT_FOUND")]));
    };
    let location = text(record, "location").unwrap_or_default().to_string();
    let name = ctx
        .arg("input")
        .and_then(|input| input.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !name_ok(&name) {
        return Ok(payload(empty, &[coded(&["input", "name"], "INVALID_NAME")]));
    }
    if drawer_named(state, &location, &name, Some(&id)).is_some() {
        return Ok(payload(
            empty,
            &[coded(&["input", "name"], "CASH_DRAWER_ALREADY_EXISTS")],
        ));
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("name".into(), json!(name.trim()));
    }
    state.emit("cash_drawers/update", &id);
    Ok(payload(json!({"cashDrawer": id}), &[]))
}

fn assign(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"pointOfSaleDevice": null});
    let state = mutation.state();
    let drawer = ctx.arg_str("cashDrawerId").unwrap_or_default().to_string();
    let device = ctx
        .arg_str("pointOfSaleDeviceId")
        .unwrap_or_default()
        .to_string();
    let Some(drawer_record) = state.get_of(&drawer, "CashDrawer") else {
        return Ok(payload(
            empty,
            &[coded(&["cashDrawerId"], "CASH_DRAWER_NOT_FOUND")],
        ));
    };
    let Some(device_record) = state.get_of(&device, "PointOfSaleDevice") else {
        return Ok(payload(
            empty,
            &[coded(
                &["pointOfSaleDeviceId"],
                "POINT_OF_SALE_DEVICE_NOT_FOUND",
            )],
        ));
    };
    if text(drawer_record, "location") != text(device_record, DEVICE_LOCATION) {
        return Ok(payload(
            empty,
            &[coded(&["cashDrawerId"], "LOCATION_MISMATCH")],
        ));
    }
    assign_device(mutation, ctx, &device, &drawer);
    Ok(payload(json!({"pointOfSaleDevice": device}), &[]))
}

/// Whether a reason code of any kind has a code (ignoring case).
fn code_taken(state: &ShopState, code: &str) -> bool {
    REASON_CODE_TYPES.iter().any(|type_name| {
        state.all(type_name).any(|record| {
            text(record, "code").is_some_and(|known| known.eq_ignore_ascii_case(code))
        })
    })
}

fn reason_code_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"reasonCode": null});
    let code = ctx.arg_str("code").unwrap_or_default().trim().to_string();
    if code.is_empty() {
        return Ok(payload(empty, &[coded(&["code"], "BLANK")]));
    }
    if code_taken(mutation.state(), &code) {
        return Ok(payload(empty, &[coded(&["code"], "CODE_ALREADY_EXISTS")]));
    }
    let state = mutation.state_mut();
    let id = state.new_gid("CashManagementCustomReasonCode");
    state.insert(value::object(json!({
        "__typename": "CashManagementCustomReasonCode",
        "id": id,
        "code": code,
    })));
    state.emit("cash_management_reason_codes/create", &id);
    Ok(payload(json!({"reasonCode": id}), &[]))
}

fn reason_code_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"deletedId": null});
    let state = mutation.state();
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if !is_reason_code(state, &id) {
        return Ok(payload(empty, &[coded(&["id"], "NOT_FOUND")]));
    }
    if state.type_of(&id) != Some("CashManagementCustomReasonCode") {
        return Ok(payload(empty, &[coded(&["id"], "NOT_DELETABLE")]));
    }
    mutation
        .state_mut()
        .delete(&id, "cash_management_reason_codes/delete");
    Ok(payload(json!({"deletedId": id}), &[]))
}

/// `cashManagementReasonCodes`: the default codes, the system ones, then the custom ones.
fn reason_codes(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let items: Vec<Value> = REASON_CODE_TYPES
        .iter()
        .flat_map(|type_name| state.keys(type_name))
        .map(|key| json!(key))
        .collect();
    r.connection(ctx, items)
}

/// `CashDrawer.pointOfSaleDevices`: the devices assigned to the drawer.
fn drawer_devices(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let drawer = object.key().unwrap_or_default();
    let items: Vec<Value> = keys_with(r.state(), "PointOfSaleDevice", "cashDrawer", drawer)
        .map(|device| json!(device))
        .collect();
    r.connection(ctx, items)
}
