//! Inventory shipments: `inventoryShipmentCreate`, `inventoryShipmentCreateInTransit`,
//! `inventoryShipmentAddItems`, `inventoryShipmentRemoveItems`,
//! `inventoryShipmentUpdateItemQuantities`, `inventoryShipmentMarkInTransit`,
//! `inventoryShipmentReceive`, `inventoryShipmentSetTracking`, `inventoryShipmentSetBarcode`,
//! `inventoryShipmentDelete`, and the search filters of `inventoryShipments`.
//!
//! A shipment carries units of a transfer (`movementId`): its items must be on the transfer,
//! within what the transfer has not shipped or picked yet (`ITEM_NOT_FOUND`,
//! `INVALID_QUANTITY`) *(lsf's reading: Shopify's admin adds a shipment's new products to the
//! transfer; the API's reference does not say)*, and the transfer must be a draft, ready to ship
//! or in progress (`INVALID_TRANSFER_STATUS`).
//!
//! - **Draft**: the units are picked (`pickedForShipmentQuantity` of the transfer's lines);
//!   nothing moves. Only a draft can be deleted ("Only draft shipments can be deleted").
//! - **In transit** (`inventoryShipmentMarkInTransit`, `…CreateInTransit`): the units leave the
//!   origin (what the transfer committed first) and are `incoming` at the destination, which is
//!   stocked with the item when it was not (see `stock`); the transfer is in progress.
//!   Units removed or reduced while in transit, and not received, come back.
//! - **Received** (`inventoryShipmentReceive`): each line's units are accepted, rejected or
//!   cancelled, up to what is unreceived (`INVALID_QUANTITY`); `bulkReceiveAction` gives every
//!   unit left that reason. Accepted units are available at the destination; the shipment is
//!   `PARTIALLY_RECEIVED` until nothing is left, then `RECEIVED`; `dateReceived` is the first
//!   receipt's. `totalReceivedQuantity` counts the accepted and the rejected units *(lsf's
//!   reading of "received")*.
//! - Barcodes are unique in the shop and 255 characters at most (`BARCODE_DUPLICATE`,
//!   `BARCODE_TOO_LONG`); a tracking URL must be an `http(s)` URL (`INVALID_URL`).
//! - `@idempotent`: required as of 2026-04 on `inventoryShipmentCreate`, `…CreateInTransit`,
//!   `…AddItems` and `…Receive` ([`super::idempotent`]).
//!
//! Events: `inventory_shipments/create`, `…/add_items`, `…/remove_items`,
//! `…/update_item_quantities`, `…/mark_in_transit`, `…/receive_items`, `…/update_tracking`,
//! `…/delete`; a barcode set is `inventory_shipments/update`, which no subscription can name.
//! A shipment is named after its transfer: `T1-1`, `T1-2` (the Help Center's `T289-1`).

use serde_json::{Map, Value, json};

use super::transfers::{self, COMMITTED, SHIPPABLE, TRANSFER};
use super::{
    coded, emit_change, idempotent, int, keys_of, objects, push, snapshot_location, stock, text,
};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::search::SearchValue;
use crate::state::{Record, ShopState, gid_number, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("inventoryShipmentCreate", |m, ctx| {
            idempotent(m, ctx, json!({"inventoryShipment": null}), create)
        })
        .mutation("inventoryShipmentCreateInTransit", |m, ctx| {
            idempotent(
                m,
                ctx,
                json!({"inventoryShipment": null}),
                create_in_transit,
            )
        })
        .mutation("inventoryShipmentAddItems", |m, ctx| {
            idempotent(
                m,
                ctx,
                json!({"inventoryShipment": null, "addedItems": null}),
                add_items,
            )
        })
        .mutation("inventoryShipmentReceive", |m, ctx| {
            idempotent(m, ctx, json!({"inventoryShipment": null}), receive)
        })
        .mutation("inventoryShipmentRemoveItems", remove_items)
        .mutation("inventoryShipmentUpdateItemQuantities", update_quantities)
        .mutation("inventoryShipmentMarkInTransit", mark_in_transit)
        .mutation("inventoryShipmentSetTracking", set_tracking)
        .mutation("inventoryShipmentSetBarcode", set_barcode)
        .mutation("inventoryShipmentDelete", delete)
        .search_field(
            "InventoryShipment",
            "destination_id",
            |r, _item, shipment| {
                let destination = text(shipment, TRANSFER)
                    .and_then(|transfer| r.state().get(transfer))
                    .and_then(|transfer| snapshot_location(transfer.get("destination")));
                Some(
                    destination
                        .as_deref()
                        .and_then(SearchValue::id)
                        .into_iter()
                        .collect(),
                )
            },
        )
        .search_field(
            "InventoryShipment",
            "tracking_number",
            |_r, _item, shipment| {
                Some(
                    shipment
                        .get("tracking")
                        .and_then(|tracking| tracking.get("trackingNumber"))
                        .and_then(Value::as_str)
                        .map(|number| SearchValue::Keyword(number.to_string()))
                        .into_iter()
                        .collect(),
                )
            },
        );
}

/// The record field where a shipment line item keeps its shipment.
const SHIPMENT: &str = "lsfShipment";

/// The record field where a transfer keeps how many shipments it has had: the number of the
/// next one's name.
const SHIPMENT_COUNT: &str = "lsfShipmentCount";

/// The most characters a barcode has.
const BARCODE_LIMIT: usize = 255;

fn shipment_of(state: &ShopState, id: &str) -> Option<Record> {
    state.get_of(id, "InventoryShipment").cloned()
}

fn transfer_of(state: &ShopState, shipment: &Record) -> Option<String> {
    text(shipment, TRANSFER)
        .filter(|transfer| state.contains(transfer))
        .map(str::to_string)
}

/// The checks of a barcode, under `path`.
fn check_barcode(
    ctx: &FieldContext<'_>,
    state: &ShopState,
    barcode: Option<&str>,
    except: Option<&str>,
    path: &[&str],
    errors: &mut Vec<UserError>,
) {
    let Some(barcode) = barcode else {
        return;
    };
    if barcode.chars().count() > BARCODE_LIMIT {
        errors.push(coded(ctx, path, "BARCODE_TOO_LONG"));
    } else if !barcode.is_empty()
        && state.keys("InventoryShipment").any(|shipment| {
            Some(shipment) != except
                && state
                    .get(shipment)
                    .and_then(|record| text(record, "barcode"))
                    == Some(barcode)
        })
    {
        errors.push(coded(ctx, path, "BARCODE_DUPLICATE"));
    }
}

/// The `InventoryShipmentTracking` of an input, checked.
fn tracking_of(
    ctx: &FieldContext<'_>,
    input: Option<&Map<String, Value>>,
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Value {
    let Some(input) = input else {
        return Value::Null;
    };
    if let Some(url) = text(input, "trackingUrl")
        && !lsf_core::urls::is_http_url(url)
    {
        let mut at = path.to_vec();
        at.push("trackingUrl");
        errors.push(coded(ctx, &at, "INVALID_URL"));
    }
    let arrives = value::date_time_of(input.get("arrivesAt"))
        .map(|at| json!(value::date_time(at)))
        .unwrap_or(Value::Null);
    json!({
        "arrivesAt": arrives,
        "company": input.get("company").cloned().unwrap_or(Value::Null),
        "trackingNumber": input.get("trackingNumber").cloned().unwrap_or(Value::Null),
        "trackingUrl": input.get("trackingUrl").cloned().unwrap_or(Value::Null),
    })
}

/// `(inventory item, quantity)` of `[InventoryShipmentLineItemInput]`, checked against what the
/// transfer has left to ship. `already` is what the shipment holds of each item (its units count
/// as picked already when the shipment is a draft).
fn check_lines(
    ctx: &FieldContext<'_>,
    state: &ShopState,
    transfer: &str,
    lines: &[Map<String, Value>],
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Vec<(String, i64)> {
    let origin = state
        .get(transfer)
        .and_then(|record| transfers::locations(state, record).0);
    let mut picked: Vec<(String, i64)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let at = |field: &str| -> Vec<String> {
            path.iter()
                .map(|part| part.to_string())
                .chain([index.to_string(), field.to_string()])
                .collect()
        };
        let item = line
            .get("inventoryItemId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let quantity = line.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if picked.iter().any(|(known, _)| known == item) {
            errors.push(coded(
                ctx,
                &as_path(&at("inventoryItemId")),
                "DUPLICATE_ITEM",
            ));
            continue;
        }
        let prefix: Vec<String> = path
            .iter()
            .map(|part| part.to_string())
            .chain(std::iter::once(index.to_string()))
            .collect();
        if !transfers::check_item(ctx, state, item, origin.as_deref(), &prefix, errors) {
            continue;
        }
        let Some(line) = transfers::line_for(state, transfer, item) else {
            errors.push(coded(
                ctx,
                &as_path(&at("inventoryItemId")),
                "ITEM_NOT_FOUND",
            ));
            continue;
        };
        let processable = state
            .get(&line)
            .map_or(0, |record| int(record, "processableQuantity"));
        if quantity <= 0 || quantity > processable {
            errors.push(coded(ctx, &as_path(&at("quantity")), "INVALID_QUANTITY"));
            continue;
        }
        picked.push((item.to_string(), quantity));
    }
    picked
}

fn as_path(parts: &[String]) -> Vec<&str> {
    parts.iter().map(String::as_str).collect()
}

/// Makes a draft shipment of a transfer. Returns its GID.
fn insert_shipment(
    state: &mut ShopState,
    transfer: &str,
    barcode: Value,
    tracking: Value,
    date: Option<String>,
) -> String {
    let count = state
        .get(transfer)
        .map_or(0, |record| int(record, SHIPMENT_COUNT))
        + 1;
    let number = gid_number(transfer);
    let id = state.new_gid("InventoryShipment");
    let now = state.timestamp();
    state.insert(value::object(json!({
        "__typename": "InventoryShipment",
        "id": id,
        "name": format!("T{number}-{count}"),
        "status": "DRAFT",
        "barcode": barcode,
        "tracking": tracking,
        "dateCreated": date.unwrap_or(now),
        "dateShipped": null,
        "dateReceived": null,
        "lineItems": [],
        "lineItemTotalQuantity": 0,
        "totalAcceptedQuantity": 0,
        "totalRejectedQuantity": 0,
        "totalCanceledQuantity": 0,
        "totalReceivedQuantity": 0,
        TRANSFER: transfer,
    })));
    if let Some(record) = state.get_mut(transfer) {
        record.insert(SHIPMENT_COUNT.into(), json!(count));
    }
    push(state, transfer, "shipments", &id);
    id
}

/// Adds units of an item to a shipment: its line of the item, or a new one. Returns the line.
fn add_units(state: &mut ShopState, shipment: &str, item: &str, units: i64) -> String {
    let existing = keys_of(state, shipment, "lineItems")
        .into_iter()
        .find(|line| {
            state
                .get(line)
                .is_some_and(|record| text(record, "inventoryItem") == Some(item))
        });
    if let Some(line) = existing {
        if let Some(record) = state.get_mut(&line) {
            let quantity = int(record, "quantity") + units;
            record.insert("quantity".into(), json!(quantity));
        }
        return line;
    }
    let transfer = state
        .get(shipment)
        .and_then(|record| text(record, TRANSFER))
        .map(str::to_string);
    let line = state.new_gid("InventoryShipmentLineItem");
    state.insert(value::object(json!({
        "__typename": "InventoryShipmentLineItem",
        "id": line,
        "inventoryItem": item,
        "quantity": units,
        "acceptedQuantity": 0,
        "rejectedQuantity": 0,
        "canceledQuantity": 0,
        "unreceivedQuantity": units,
        SHIPMENT: shipment,
        TRANSFER: transfer,
    })));
    push(state, shipment, "lineItems", &line);
    line
}

/// Recomputes a shipment's totals and its lines' unreceived quantities, and its status once it
/// has been received from.
fn refresh(state: &mut ShopState, shipment: &str) {
    let lines = keys_of(state, shipment, "lineItems");
    let mut totals = [0i64; 5];
    for line in &lines {
        let Some(record) = state.get_mut(line) else {
            continue;
        };
        let quantity = int(record, "quantity");
        let accepted = int(record, "acceptedQuantity");
        let rejected = int(record, "rejectedQuantity");
        let canceled = int(record, "canceledQuantity");
        let unreceived = (quantity - accepted - rejected - canceled).max(0);
        record.insert("unreceivedQuantity".into(), json!(unreceived));
        totals[0] += quantity;
        totals[1] += accepted;
        totals[2] += rejected;
        totals[3] += canceled;
        totals[4] += unreceived;
    }
    if let Some(record) = state.get_mut(shipment) {
        let [quantity, accepted, rejected, canceled, unreceived] = totals;
        record.insert("lineItemTotalQuantity".into(), json!(quantity));
        record.insert("totalAcceptedQuantity".into(), json!(accepted));
        record.insert("totalRejectedQuantity".into(), json!(rejected));
        record.insert("totalCanceledQuantity".into(), json!(canceled));
        record.insert("totalReceivedQuantity".into(), json!(accepted + rejected));
        let status = text(record, "status").unwrap_or("DRAFT").to_string();
        if matches!(
            status.as_str(),
            "IN_TRANSIT" | "PARTIALLY_RECEIVED" | "RECEIVED"
        ) && accepted + rejected + canceled > 0
        {
            let new = if unreceived == 0 {
                "RECEIVED"
            } else {
                "PARTIALLY_RECEIVED"
            };
            record.insert("status".into(), json!(new));
        }
    }
}

/// Moves units of a shipment's item between the origin and the destination: positive, they
/// ship (out of the origin, what the transfer's line committed first; `incoming` at the
/// destination); negative, they come back.
fn move_units(state: &mut ShopState, transfer: &str, item: &str, units: i64) {
    if units == 0 {
        return;
    }
    let Some(record) = state.get(transfer).cloned() else {
        return;
    };
    let (origin, destination) = transfers::locations(state, &record);
    if let Some(origin) = &origin {
        let line = transfers::line_for(state, transfer, item);
        let committed = line
            .as_deref()
            .and_then(|line| state.get(line))
            .map_or(0, |record| int(record, COMMITTED));
        let from_committed = if units > 0 { units.min(committed) } else { 0 };
        stock::ship_out(state, item, origin, from_committed, units);
        if let Some(line) = line
            && let Some(record) = state.get_mut(&line)
        {
            record.insert(COMMITTED.into(), json!(committed - from_committed));
        }
    }
    if let Some(destination) = &destination {
        stock::incoming(state, item, destination, units);
    }
}

/// Puts a shipment in transit: its units leave the origin for the destination.
fn ship(state: &mut ShopState, shipment: &str, date: Option<String>) {
    let Some(record) = state.get(shipment).cloned() else {
        return;
    };
    let Some(transfer) = transfer_of(state, &record) else {
        return;
    };
    let now = state.timestamp();
    if let Some(record) = state.get_mut(shipment) {
        record.insert("status".into(), json!("IN_TRANSIT"));
        record.insert("dateShipped".into(), json!(date.unwrap_or(now)));
    }
    for line in gids(record.get("lineItems")) {
        let Some(line) = state.get(&line).cloned() else {
            continue;
        };
        if let Some(item) = text(&line, "inventoryItem") {
            move_units(state, &transfer, item, int(&line, "quantity"));
        }
    }
    transfers::refresh(state, &transfer);
    emit_change(
        state,
        "inventory_shipments/mark_in_transit",
        shipment,
        json!({}),
    );
}

/// Removes a shipment, its lines with it (a draft: nothing moved), with
/// `inventory_shipments/delete`.
pub fn remove_shipment(state: &mut ShopState, shipment: &str) {
    let Some(record) = state.get(shipment).cloned() else {
        return;
    };
    state.delete(shipment, "inventory_shipments/delete");
    for line in gids(record.get("lineItems")) {
        state.remove(&line);
    }
    if let Some(transfer) = transfer_of(state, &record) {
        state.pull_key(&transfer, "shipments", shipment);
        transfers::refresh(state, &transfer);
    }
}

// --- The mutations ---

fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    create_shipment(mutation, ctx, false)
}

fn create_in_transit(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    create_shipment(mutation, ctx, true)
}

fn create_shipment(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    in_transit: bool,
) -> Result<Value, FieldError> {
    let empty = json!({"inventoryShipment": null});
    let input = ctx.arg_object_or_default("input");
    let state = mutation.state();
    let transfer_id = text(&input, "movementId").unwrap_or_default().to_string();
    let Some(transfer) = transfers::find(state, &transfer_id) else {
        return Ok(payload(
            empty,
            &[coded(ctx, &["input", "movementId"], "TRANSFER_NOT_FOUND")],
        ));
    };
    if !SHIPPABLE.contains(&text(&transfer, "status").unwrap_or_default()) {
        return Ok(payload(
            empty,
            &[coded(
                ctx,
                &["input", "movementId"],
                "INVALID_TRANSFER_STATUS",
            )],
        ));
    }
    let mut errors = Vec::new();
    let lines = objects(input.get("lineItems"));
    if in_transit && lines.is_empty() {
        errors.push(coded(ctx, &["input", "lineItems"], "ITEMS_EMPTY"));
    }
    let picked = check_lines(
        ctx,
        state,
        &transfer_id,
        &lines,
        &["input", "lineItems"],
        &mut errors,
    );
    let barcode = text(&input, "barcode");
    check_barcode(
        ctx,
        state,
        barcode,
        None,
        &["input", "barcode"],
        &mut errors,
    );
    let tracking_input = input.get("trackingInput").and_then(Value::as_object);
    let tracking = tracking_of(
        ctx,
        tracking_input,
        &["input", "trackingInput"],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let date = value::date_time_of(input.get("dateCreated")).map(value::date_time);
    let state = mutation.state_mut();
    let shipment = insert_shipment(
        state,
        &transfer_id,
        barcode.map_or(Value::Null, |barcode| json!(barcode)),
        tracking,
        date,
    );
    for (item, quantity) in &picked {
        add_units(state, &shipment, item, *quantity);
    }
    refresh(state, &shipment);
    transfers::refresh(state, &transfer_id);
    emit_change(state, "inventory_shipments/create", &shipment, json!({}));
    if in_transit {
        ship(state, &shipment, None);
    }
    Ok(payload(json!({"inventoryShipment": shipment}), &[]))
}

/// The shipment of the `id` argument, or the payload of `SHIPMENT_NOT_FOUND`.
fn shipment_arg(
    ctx: &FieldContext<'_>,
    state: &ShopState,
    empty: &Value,
) -> Result<(String, Record), Value> {
    let id = ctx.arg_str("id").unwrap_or_default();
    shipment_of(state, id)
        .map(|record| (id.to_string(), record))
        .ok_or_else(|| payload(empty.clone(), &[coded(ctx, &["id"], "SHIPMENT_NOT_FOUND")]))
}

/// `inventoryShipmentAddItems`: to a draft (the units are picked) or a shipment in transit (they
/// ship at once).
fn add_items(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"inventoryShipment": null, "addedItems": null});
    let (id, shipment) = match shipment_arg(ctx, mutation.state(), &empty) {
        Ok(found) => found,
        Err(answer) => return Ok(answer),
    };
    let state = mutation.state();
    let status = text(&shipment, "status").unwrap_or_default().to_string();
    let Some(transfer) =
        transfer_of(state, &shipment).filter(|_| matches!(status.as_str(), "DRAFT" | "IN_TRANSIT"))
    else {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "INVALID_SHIPMENT_STATUS")],
        ));
    };
    let mut errors = Vec::new();
    let lines = objects(ctx.arg("lineItems"));
    let picked = check_lines(ctx, state, &transfer, &lines, &["lineItems"], &mut errors);
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let mut added = Vec::new();
    let mut context = Vec::new();
    for (item, quantity) in &picked {
        let line = add_units(state, &id, item, *quantity);
        if status == "IN_TRANSIT" {
            move_units(state, &transfer, item, *quantity);
        }
        context.push(json!({"id": line, "quantity": quantity}));
        if !added.contains(&json!(line)) {
            added.push(json!(line));
        }
    }
    refresh(state, &id);
    transfers::refresh(state, &transfer);
    emit_change(
        state,
        "inventory_shipments/add_items",
        &id,
        json!({"items": context}),
    );
    Ok(payload(
        json!({"inventoryShipment": id, "addedItems": added}),
        &[],
    ))
}

/// The line of a shipment an id names, with the errors of one that is not.
fn shipment_line(
    ctx: &FieldContext<'_>,
    state: &ShopState,
    shipment: &Record,
    line: &str,
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Option<Record> {
    let found = gids(shipment.get("lineItems"))
        .iter()
        .any(|known| known == line)
        .then(|| state.get(line).cloned())
        .flatten();
    if found.is_none() {
        errors.push(coded(ctx, path, "ITEM_NOT_FOUND"));
    }
    found
}

/// Changes the units of a shipment's line to `quantity` (none removes it): what changed comes
/// back to the origin, or leaves it, when the shipment is in transit. Only units not received
/// can change.
fn set_units(state: &mut ShopState, shipment: &str, line: &str, quantity: i64) {
    let Some(record) = state.get(shipment).cloned() else {
        return;
    };
    let Some(line_record) = state.get(line).cloned() else {
        return;
    };
    let delta = quantity - int(&line_record, "quantity");
    if text(&record, "status") != Some("DRAFT")
        && let (Some(transfer), Some(item)) = (
            transfer_of(state, &record),
            text(&line_record, "inventoryItem"),
        )
    {
        move_units(state, &transfer, item, delta);
    }
    if quantity == 0 {
        state.pull_key(shipment, "lineItems", line);
        state.remove(line);
    } else if let Some(record) = state.get_mut(line) {
        record.insert("quantity".into(), json!(quantity));
    }
}

/// `inventoryShipmentRemoveItems`: lines of a draft, or of a shipment in transit that have
/// received nothing.
fn remove_items(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"inventoryShipment": null});
    let (id, shipment) = match shipment_arg(ctx, mutation.state(), &empty) {
        Ok(found) => found,
        Err(answer) => return Ok(answer),
    };
    let state = mutation.state();
    if !matches!(text(&shipment, "status"), Some("DRAFT" | "IN_TRANSIT")) {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "INVALID_SHIPMENT_STATUS")],
        ));
    }
    let asked = gids(ctx.arg("lineItems"));
    let mut errors = Vec::new();
    for (index, line) in asked.iter().enumerate() {
        let index = index.to_string();
        let path = ["lineItems", index.as_str()];
        if let Some(record) = shipment_line(ctx, state, &shipment, line, &path, &mut errors)
            && int(&record, "unreceivedQuantity") < int(&record, "quantity")
        {
            errors.push(coded(ctx, &path, "INVALID_SHIPMENT_STATUS"));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let mut removed = Vec::new();
    for line in &asked {
        set_units(state, &id, line, 0);
        removed.push(json!({"id": line}));
    }
    refresh(state, &id);
    if let Some(transfer) = transfer_of(state, &shipment) {
        transfers::refresh(state, &transfer);
    }
    emit_change(
        state,
        "inventory_shipments/remove_items",
        &id,
        json!({"items": removed}),
    );
    Ok(payload(json!({"inventoryShipment": id}), &[]))
}

fn update_quantities(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"shipment": null, "updatedLineItems": null});
    let (id, shipment) = match shipment_arg(ctx, mutation.state(), &empty) {
        Ok(found) => found,
        Err(answer) => return Ok(answer),
    };
    let state = mutation.state();
    if !matches!(text(&shipment, "status"), Some("DRAFT" | "IN_TRANSIT")) {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "INVALID_SHIPMENT_STATUS")],
        ));
    }
    let transfer = transfer_of(state, &shipment).unwrap_or_default();
    let items = objects(ctx.arg("items"));
    let mut errors = Vec::new();
    let mut changes: Vec<(String, i64, i64)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let index = index.to_string();
        let line = text(item, "shipmentLineItemId").unwrap_or_default();
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        let Some(record) = shipment_line(
            ctx,
            state,
            &shipment,
            line,
            &["items", index.as_str(), "shipmentLineItemId"],
            &mut errors,
        ) else {
            continue;
        };
        let old = int(&record, "quantity");
        let received = old - int(&record, "unreceivedQuantity");
        let left = text(&record, "inventoryItem")
            .and_then(|item| transfers::line_for(state, &transfer, item))
            .and_then(|line| state.get(&line))
            .map_or(0, |line| int(line, "processableQuantity"));
        if quantity < received.max(1) || quantity - old > left {
            errors.push(coded(
                ctx,
                &["items", index.as_str(), "quantity"],
                "INVALID_QUANTITY",
            ));
            continue;
        }
        changes.push((line.to_string(), old, quantity));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let mut updated = Vec::new();
    let mut context = Vec::new();
    for (line, old, new) in &changes {
        set_units(state, &id, line, *new);
        updated.push(json!(line));
        context.push(json!({"id": line, "old_quantity": old, "new_quantity": new}));
    }
    refresh(state, &id);
    transfers::refresh(state, &transfer);
    emit_change(
        state,
        "inventory_shipments/update_item_quantities",
        &id,
        json!({"items": context}),
    );
    Ok(payload(
        json!({"shipment": id, "updatedLineItems": updated}),
        &[],
    ))
}

fn mark_in_transit(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"inventoryShipment": null});
    let (id, shipment) = match shipment_arg(ctx, mutation.state(), &empty) {
        Ok(found) => found,
        Err(answer) => return Ok(answer),
    };
    let state = mutation.state();
    if text(&shipment, "status") != Some("DRAFT") || transfer_of(state, &shipment).is_none() {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "INVALID_SHIPMENT_STATUS")],
        ));
    }
    if int(&shipment, "lineItemTotalQuantity") == 0 {
        return Ok(payload(empty, &[coded(ctx, &["id"], "ITEMS_EMPTY")]));
    }
    let date = ctx
        .arg_str("dateShipped")
        .and_then(value::parse_date_time)
        .map(value::date_time);
    ship(mutation.state_mut(), &id, date);
    Ok(payload(json!({"inventoryShipment": id}), &[]))
}

fn receive(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"inventoryShipment": null});
    let (id, shipment) = match shipment_arg(ctx, mutation.state(), &empty) {
        Ok(found) => found,
        Err(answer) => return Ok(answer),
    };
    let state = mutation.state();
    let Some(transfer) = transfer_of(state, &shipment).filter(|_| {
        matches!(
            text(&shipment, "status"),
            Some("IN_TRANSIT" | "PARTIALLY_RECEIVED")
        )
    }) else {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "INVALID_SHIPMENT_STATUS")],
        ));
    };
    let mut errors = Vec::new();
    // What each line receives, by reason: (accepted, rejected, canceled).
    let mut receipts: Vec<(String, [i64; 3])> = Vec::new();
    let items = objects(ctx.arg("lineItems"));
    for (index, item) in items.iter().enumerate() {
        let index = index.to_string();
        let line = text(item, "shipmentLineItemId").unwrap_or_default();
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if shipment_line(
            ctx,
            state,
            &shipment,
            line,
            &["lineItems", index.as_str(), "shipmentLineItemId"],
            &mut errors,
        )
        .is_none()
        {
            continue;
        }
        if quantity <= 0 {
            errors.push(coded(
                ctx,
                &["lineItems", index.as_str(), "quantity"],
                "INVALID_QUANTITY",
            ));
            continue;
        }
        take(
            &mut receipts,
            line,
            text(item, "reason").unwrap_or("ACCEPTED"),
            quantity,
        );
    }
    for (line, counts) in &receipts {
        let unreceived = state
            .get(line)
            .map_or(0, |record| int(record, "unreceivedQuantity"));
        if counts.iter().sum::<i64>() > unreceived {
            errors.push(coded(ctx, &["lineItems"], "INVALID_QUANTITY"));
            break;
        }
    }
    if let Some(bulk) = ctx.arg_str("bulkReceiveAction") {
        for line in gids(shipment.get("lineItems")) {
            let unreceived = state
                .get(&line)
                .map_or(0, |record| int(record, "unreceivedQuantity"));
            let asked: i64 = receipts
                .iter()
                .filter(|(known, _)| *known == line)
                .map(|(_, counts)| counts.iter().sum::<i64>())
                .sum();
            if unreceived > asked {
                take(&mut receipts, &line, bulk, unreceived - asked);
            }
        }
    }
    if items.is_empty() && ctx.arg("bulkReceiveAction").is_none() {
        errors.push(coded(ctx, &["lineItems"], "INVALID_SHIPMENT_INPUT"));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let destination = state
        .get(&transfer)
        .and_then(|record| transfers::locations(state, record).1);
    let date = ctx
        .arg_str("dateReceived")
        .and_then(value::parse_date_time)
        .map(value::date_time);
    let state = mutation.state_mut();
    let mut context = Vec::new();
    for (line, [accepted, rejected, canceled]) in &receipts {
        let Some(record) = state.get(line).cloned() else {
            continue;
        };
        let old = [
            int(&record, "acceptedQuantity"),
            int(&record, "rejectedQuantity"),
            int(&record, "canceledQuantity"),
        ];
        let new = [old[0] + accepted, old[1] + rejected, old[2] + canceled];
        if let Some(record) = state.get_mut(line) {
            record.insert("acceptedQuantity".into(), json!(new[0]));
            record.insert("rejectedQuantity".into(), json!(new[1]));
            record.insert("canceledQuantity".into(), json!(new[2]));
        }
        if let (Some(destination), Some(item)) = (&destination, text(&record, "inventoryItem")) {
            stock::receive(state, item, destination, *accepted, rejected + canceled);
        }
        let remaining = int(&record, "quantity") - new.iter().sum::<i64>();
        context.push(json!({
            "id": line,
            "old_accepted_quantity": old[0],
            "old_rejected_quantity": old[1],
            "old_canceled_quantity": old[2],
            "new_accepted_quantity": new[0],
            "new_rejected_quantity": new[1],
            "new_canceled_quantity": new[2],
            "remaining_quantity_to_receive": remaining,
        }));
    }
    let now = state.timestamp();
    if let Some(record) = state.get_mut(&id)
        && record.get("dateReceived").is_none_or(Value::is_null)
    {
        record.insert("dateReceived".into(), json!(date.unwrap_or(now)));
    }
    refresh(state, &id);
    transfers::refresh(state, &transfer);
    emit_change(
        state,
        "inventory_shipments/receive_items",
        &id,
        json!({"items": context}),
    );
    Ok(payload(json!({"inventoryShipment": id}), &[]))
}

/// Adds what a line receives for a reason to the receipts.
fn take(receipts: &mut Vec<(String, [i64; 3])>, line: &str, reason: &str, quantity: i64) {
    let slot = match reason {
        "ACCEPTED" => 0,
        "REJECTED" => 1,
        _ => 2,
    };
    match receipts.iter_mut().find(|(known, _)| known == line) {
        Some((_, counts)) => counts[slot] += quantity,
        None => {
            let mut counts = [0; 3];
            counts[slot] = quantity;
            receipts.push((line.to_string(), counts));
        }
    }
}

fn set_tracking(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"inventoryShipment": null});
    let (id, _) = match shipment_arg(ctx, mutation.state(), &empty) {
        Ok(found) => found,
        Err(answer) => return Ok(answer),
    };
    let mut errors = Vec::new();
    let tracking = tracking_of(ctx, ctx.arg_object("tracking"), &["tracking"], &mut errors);
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("tracking".into(), tracking);
    }
    emit_change(state, "inventory_shipments/update_tracking", &id, json!({}));
    Ok(payload(json!({"inventoryShipment": id}), &[]))
}

fn set_barcode(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"inventoryShipment": null});
    let (id, _) = match shipment_arg(ctx, mutation.state(), &empty) {
        Ok(found) => found,
        Err(answer) => return Ok(answer),
    };
    let barcode = ctx.arg_str("barcode").unwrap_or_default();
    let mut errors = Vec::new();
    check_barcode(
        ctx,
        mutation.state(),
        Some(barcode),
        Some(&id),
        &["barcode"],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("barcode".into(), json!(barcode));
    }
    emit_change(state, "inventory_shipments/update", &id, json!({}));
    Ok(payload(json!({"inventoryShipment": id}), &[]))
}

/// `inventoryShipmentDelete`: a draft only.
fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"id": null});
    let (id, shipment) = match shipment_arg(ctx, mutation.state(), &empty) {
        Ok(found) => found,
        Err(answer) => return Ok(answer),
    };
    if text(&shipment, "status") != Some("DRAFT") {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "INVALID_SHIPMENT_STATUS")],
        ));
    }
    remove_shipment(mutation.state_mut(), &id);
    Ok(payload(json!({"id": id}), &[]))
}
