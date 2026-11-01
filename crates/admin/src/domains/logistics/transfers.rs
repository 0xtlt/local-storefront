//! Inventory transfers: `inventoryTransferCreate`, `inventoryTransferCreateAsReadyToShip`,
//! `inventoryTransferDuplicate`, `inventoryTransferEdit`, `inventoryTransferSetItems`,
//! `inventoryTransferRemoveItems`, `inventoryTransferMarkAsReadyToShip`,
//! `inventoryTransferCancel`, `inventoryTransferDelete`, and the sort keys and search filters of
//! `inventoryTransfers`.
//!
//! As shopify.dev's reference of 2026-10 (each mutation's description) and the Help Center's
//! "Creating and processing inventory transfers" describe them:
//!
//! - A transfer is created as a **draft**: nothing is committed. Its origin and destination are
//!   `LocationSnapshot`s (the name and address as they were, with a link to the live location);
//!   either may be blank ("This can be useful when you receive inventory from an external
//!   supplier"). The items must track their inventory (`UNTRACKED_ITEM`), be stocked at the
//!   origin (`INVENTORY_STATE_NOT_ACTIVE`), not be bundles (`BUNDLED_ITEM`), appear once
//!   (`DUPLICATE_ITEM`).
//! - **Ready to ship** commits the units at the origin ("The inventory that you're transferring
//!   is committed at the origin location"); it needs an origin (`LOCATION_REQUIRED`) and items
//!   (`ITEMS_EMPTY`). Shipments then take the units: see `shipments`.
//! - `inventoryTransferSetItems` adds the items not on the transfer and replaces the
//!   `processableQuantity` of the others ("Any quantity outside the processable portion … is
//!   preserved"); `0` is allowed on a draft only (`INVALID_QUANTITY`), and leaves the line.
//! - `inventoryTransferRemoveItems` removes lines, or reduces them to what shipments hold ("the
//!   line item remains on the transfer with its quantity reduced to the allocated portion"); on
//!   a ready-to-ship transfer the units go back to `available`, and its last line cannot go
//!   (`CANT_REMOVE_ALL_ITEMS_FROM_READY_TO_SHIP_TRANSFER`).
//! - `inventoryTransferEdit` changes the reference name, note, tags, date and metafields; the
//!   locations of a draft only (`TRANSFER_LOCATION_IMMUTABLE`: "Only Draft Transfers can mutate
//!   their locations").
//! - **Canceled**: "Your committed items are made Available at the origin location"; not once a
//!   shipment exists (`SHIPMENT_EXISTS`). **Deleted**: drafts only ("You can delete inventory
//!   transfers that are in Draft status"), with their draft shipments.
//! - **Transferred** once every unit shipped and every shipment was received
//!   (`inventory_transfers/complete`).
//! - `@idempotent`: required as of 2026-04 on `inventoryTransferCreate`,
//!   `…CreateAsReadyToShip`, `…Duplicate` and `…SetItems` ([`super::idempotent`]).
//!
//! Events (the topics of `WebhookSubscriptionTopic`): `inventory_transfers/add_items`,
//! `…/update_item_quantities`, `…/remove_items`, `…/ready_to_ship`, `…/cancel`, `…/complete`,
//! `…/updated`. Shopify has no topic for a transfer created or deleted: those are emitted as
//! `inventory_transfers/create` and `…/delete`, which no subscription can name. The messages are
//! the descriptions of each payload's code enum ([`super::coded`]); the names are lsf's: `#T1`
//! for the first transfer *(chosen: the Help Center names a shipment `T289-1`)*.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value, json};

use super::{
    coded, emit_change, idempotent, int, keys_of, location_snapshot, objects, push,
    snapshot_location, stock, text,
};
use crate::domains::catalog::{self, gids};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::sort::SortValue;
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::search::SearchValue;
use crate::state::{Record, ShopState, gid_number, parse_gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("inventoryTransferCreate", |m, ctx| {
            idempotent(m, ctx, json!({"inventoryTransfer": null}), create)
        })
        .mutation("inventoryTransferCreateAsReadyToShip", |m, ctx| {
            idempotent(m, ctx, json!({"inventoryTransfer": null}), create_ready)
        })
        .mutation("inventoryTransferDuplicate", |m, ctx| {
            idempotent(m, ctx, json!({"inventoryTransfer": null}), duplicate)
        })
        .mutation("inventoryTransferSetItems", |m, ctx| {
            idempotent(
                m,
                ctx,
                json!({"inventoryTransfer": null, "updatedLineItems": null}),
                set_items,
            )
        })
        .mutation("inventoryTransferEdit", edit)
        .mutation("inventoryTransferRemoveItems", remove_items)
        .mutation("inventoryTransferMarkAsReadyToShip", mark_ready)
        .mutation("inventoryTransferCancel", cancel)
        .mutation("inventoryTransferDelete", delete)
        .sort_key("InventoryTransfer", "CREATED_AT", |r, item| {
            SortValue::of(r.item_map(item).and_then(|t| t.get("dateCreated")))
        })
        .sort_key("InventoryTransfer", "ORIGIN_NAME", |r, item| {
            SortValue::of(r.item_map(item).and_then(|t| t.get("origin")?.get("name")))
        })
        .sort_key("InventoryTransfer", "DESTINATION_NAME", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|t| t.get("destination")?.get("name")),
            )
        })
        .sort_key(
            "InventoryTransfer",
            "EXPECTED_SHIPMENT_ARRIVAL",
            |r, item| {
                let arrives = r.item_map(item).and_then(|transfer| {
                    gids(transfer.get("shipments"))
                        .iter()
                        .filter_map(|shipment| {
                            r.state()
                                .get(shipment)?
                                .get("tracking")?
                                .get("arrivesAt")?
                                .as_str()
                        })
                        .min()
                        .map(str::to_string)
                });
                SortValue::of(arrives.map(Value::String).as_ref())
            },
        )
        .search_field("InventoryTransfer", "created_at", |_r, _item, transfer| {
            Some(
                date_value(transfer.get("dateCreated"))
                    .into_iter()
                    .collect(),
            )
        })
        .search_field("InventoryTransfer", "origin_id", |_r, _item, transfer| {
            Some(id_values(snapshot_location(transfer.get("origin"))))
        })
        .search_field(
            "InventoryTransfer",
            "destination_id",
            |_r, _item, transfer| Some(id_values(snapshot_location(transfer.get("destination")))),
        )
        .search_field(
            "InventoryTransfer",
            "product_variant_id",
            |r, _item, transfer| {
                Some(
                    items_of(r.state(), transfer)
                        .iter()
                        .filter_map(|item| {
                            text(r.state().get(item)?, "variant").map(str::to_string)
                        })
                        .flat_map(|variant| id_values(Some(variant)))
                        .collect(),
                )
            },
        )
        .search_field("InventoryTransfer", "product_id", |r, _item, transfer| {
            Some(
                items_of(r.state(), transfer)
                    .iter()
                    .filter_map(|item| {
                        let variant = text(r.state().get(item)?, "variant")?;
                        text(r.state().get(variant)?, "product").map(str::to_string)
                    })
                    .flat_map(|product| id_values(Some(product)))
                    .collect(),
            )
        })
        .default_text("InventoryTransfer", |_r, transfer| {
            ["name", "referenceName", "note"]
                .iter()
                .filter_map(|field| text(transfer, field).map(str::to_string))
                .chain(
                    transfer
                        .get("tags")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_string),
                )
                .collect()
        });
}

/// The record field where a transfer keeps whether its units are committed at the origin: it
/// was marked ready to ship (a draft moved straight to in transit commits nothing).
pub const COMMITS: &str = "lsfCommits";

/// The record field where a transfer line item keeps how many of its units are committed at the
/// origin.
pub const COMMITTED: &str = "lsfCommitted";

/// The record field where a line item (of a transfer, or of a shipment) keeps its transfer.
pub const TRANSFER: &str = "lsfTransfer";

/// The statuses a transfer takes shipments in.
pub const SHIPPABLE: &[&str] = &["DRAFT", "READY_TO_SHIP", "IN_PROGRESS"];

fn id_values(gid: Option<String>) -> Vec<SearchValue> {
    gid.as_deref()
        .and_then(parse_gid)
        .map(|(_, id)| SearchValue::Id(id))
        .into_iter()
        .collect()
}

fn date_value(value: Option<&Value>) -> Option<SearchValue> {
    let text = value?.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|date| SearchValue::Date(date.with_timezone(&Utc)))
}

/// The inventory items of a transfer's lines.
fn items_of(state: &ShopState, transfer: &Record) -> Vec<String> {
    gids(transfer.get("lineItems"))
        .iter()
        .filter_map(|line| text(state.get(line)?, "inventoryItem").map(str::to_string))
        .collect()
}

/// The transfer of an id, when it is one.
pub fn find(state: &ShopState, id: &str) -> Option<Record> {
    state.get_of(id, "InventoryTransfer").cloned()
}

/// The line of a transfer for an inventory item.
pub fn line_for(state: &ShopState, transfer: &str, item: &str) -> Option<String> {
    keys_of(state, transfer, "lineItems")
        .into_iter()
        .find(|line| {
            state
                .get(line)
                .is_some_and(|record| text(record, "inventoryItem") == Some(item))
        })
}

/// The origin and destination locations of a transfer (live, when they still exist).
pub fn locations(state: &ShopState, transfer: &Record) -> (Option<String>, Option<String>) {
    let live = |field: &str| {
        snapshot_location(transfer.get(field)).filter(|location| state.contains(location))
    };
    (live("origin"), live("destination"))
}

/// Moves the open transfers (not canceled nor transferred) that start or end at a location to
/// another, as `locationDeactivate` relocates "incoming movements" with the stock
/// (`FAILED_TO_RELOCATE_INCOMING_MOVEMENTS` is what Shopify answers when it cannot): their
/// snapshot names the new location, so that what they commit, ship and receive from then on
/// does so there, with `inventory_transfers/updated`.
pub fn relocate(state: &mut ShopState, from: &str, to: &str) {
    let open: Vec<String> = state
        .all("InventoryTransfer")
        .filter(|transfer| {
            !matches!(
                text(transfer, "status"),
                Some("CANCELED" | "TRANSFERRED" | "OTHER")
            ) && ["origin", "destination"]
                .iter()
                .any(|field| snapshot_location(transfer.get(*field)).as_deref() == Some(from))
        })
        .filter_map(|transfer| text(transfer, "id").map(str::to_string))
        .collect();
    for id in open {
        let snapshot = location_snapshot(state, Some(to));
        if let Some(record) = state.get_mut(&id) {
            for field in ["origin", "destination"] {
                if snapshot_location(record.get(field)).as_deref() == Some(from) {
                    record.insert(field.into(), snapshot.clone());
                }
            }
        }
        state.touch(&id);
        emit_change(state, "inventory_transfers/updated", &id, json!({}));
    }
}

// --- Checks ---

/// A location input: `Ok(None)` when blank, the errors of one that is not a location or is not
/// active.
fn check_location(
    ctx: &FieldContext<'_>,
    state: &ShopState,
    id: Option<&str>,
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Option<String> {
    let id = id?;
    if state.type_of(id) != Some("Location") {
        errors.push(coded(ctx, path, "LOCATION_NOT_FOUND"));
        return None;
    }
    if !catalog::is_active_location(state, id) {
        errors.push(coded(ctx, path, "LOCATION_NOT_ACTIVE"));
        return None;
    }
    Some(id.to_string())
}

/// The checks of an item to transfer from `origin`: an inventory item, tracked, not a bundle,
/// stocked at the origin. `path` is the item input's.
pub fn check_item(
    ctx: &FieldContext<'_>,
    state: &ShopState,
    item: &str,
    origin: Option<&str>,
    path: &[String],
    errors: &mut Vec<UserError>,
) -> bool {
    let at = |field: &'static str| -> Vec<&str> {
        path.iter()
            .map(String::as_str)
            .chain(std::iter::once(field))
            .collect()
    };
    let Some(record) = state.get_of(item, "InventoryItem") else {
        errors.push(coded(ctx, &at("inventoryItemId"), "ITEM_NOT_FOUND"));
        return false;
    };
    let bundle = text(record, "variant")
        .and_then(|variant| state.get(variant))
        .and_then(|variant| variant.get("requiresComponents"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if bundle {
        errors.push(coded(ctx, &at("inventoryItemId"), "BUNDLED_ITEM"));
        return false;
    }
    if !stock::tracked(state, item) {
        errors.push(coded(ctx, &at("inventoryItemId"), "UNTRACKED_ITEM"));
        return false;
    }
    if let Some(origin) = origin
        && !stock::stocked(state, item, origin)
    {
        errors.push(coded(
            ctx,
            &at("inventoryItemId"),
            "INVENTORY_STATE_NOT_ACTIVE",
        ));
        return false;
    }
    true
}

/// The `(inventory item, quantity)` of `[InventoryTransferLineItemInput]`, checked. `path` is the
/// list's; `zero` whether a quantity of 0 is allowed.
fn check_lines(
    ctx: &FieldContext<'_>,
    state: &ShopState,
    lines: &[Map<String, Value>],
    origin: Option<&str>,
    path: &[&str],
    zero: bool,
    errors: &mut Vec<UserError>,
) -> Vec<(String, i64)> {
    let mut picked: Vec<(String, i64)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let at: Vec<String> = path
            .iter()
            .map(|part| part.to_string())
            .chain(std::iter::once(index.to_string()))
            .collect();
        let item = line
            .get("inventoryItemId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let quantity = line.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if picked.iter().any(|(known, _)| known == item) {
            let mut field: Vec<&str> = at.iter().map(String::as_str).collect();
            field.push("inventoryItemId");
            errors.push(coded(ctx, &field, "DUPLICATE_ITEM"));
            continue;
        }
        if quantity < 0 || (quantity == 0 && !zero) {
            let mut field: Vec<&str> = at.iter().map(String::as_str).collect();
            field.push("quantity");
            errors.push(coded(ctx, &field, "INVALID_QUANTITY"));
            continue;
        }
        if check_item(ctx, state, item, origin, &at, errors) {
            picked.push((item.to_string(), quantity));
        }
    }
    picked
}

/// The checks of tags: none longer than 255 characters (Shopify's limit for a tag), none
/// blank.
fn check_tags(ctx: &FieldContext<'_>, tags: &[String], errors: &mut Vec<UserError>) {
    if tags.iter().any(|tag| tag.chars().count() > 255) {
        errors.push(coded(ctx, &["input", "tags"], "TAG_EXCEEDS_MAX_LENGTH"));
    } else if tags.iter().any(|tag| tag.trim().is_empty()) {
        errors.push(coded(ctx, &["input", "tags"], "INVALID_TAG"));
    }
}

fn tags_of(input: &Map<String, Value>) -> Option<Vec<String>> {
    input.get("tags").and_then(Value::as_array).map(|tags| {
        tags.iter()
            .filter_map(Value::as_str)
            .map(|tag| tag.trim().to_string())
            .collect()
    })
}

/// The metafield inputs of a transfer, checked as `metafieldsSet` checks them.
fn check_metafields(
    ctx: &FieldContext<'_>,
    state: &ShopState,
    owner: Option<&str>,
    input: &Map<String, Value>,
    errors: &mut Vec<UserError>,
) -> Vec<Map<String, Value>> {
    let metafields = objects(input.get("metafields"));
    let refs: Vec<&Map<String, Value>> = metafields.iter().collect();
    if !refs.is_empty()
        && !catalog::check_metafields(
            state,
            owner,
            "TRANSFER",
            &refs,
            &["input", "metafields"],
            None,
        )
        .is_empty()
    {
        errors.push(coded(ctx, &["input", "metafields"], "INVALID_METAFIELDS"));
    }
    metafields
}

// --- Records ---

/// What a new transfer is made of.
struct NewTransfer {
    origin: Option<String>,
    destination: Option<String>,
    lines: Vec<(String, i64)>,
    note: Value,
    reference: Value,
    tags: Vec<String>,
    date: Option<String>,
}

/// Makes a draft transfer and its lines. Returns its GID.
fn insert_transfer(state: &mut ShopState, new: &NewTransfer) -> String {
    let id = state.new_gid("InventoryTransfer");
    let number = gid_number(&id);
    let now = state.timestamp();
    let record = value::object(json!({
        "__typename": "InventoryTransfer",
        "id": id,
        "name": format!("#T{number}"),
        "referenceName": new.reference,
        "note": new.note,
        "tags": new.tags,
        "status": "DRAFT",
        "dateCreated": new.date.clone().unwrap_or_else(|| now.clone()),
        "origin": location_snapshot(state, new.origin.as_deref()),
        "destination": location_snapshot(state, new.destination.as_deref()),
        "lineItems": [],
        "shipments": [],
        "totalQuantity": 0,
        "receivedQuantity": 0,
        "hasTimelineComment": false,
        "events": [],
        "metafields": [],
        COMMITS: false,
    }));
    state.insert(record);
    for (item, quantity) in &new.lines {
        add_line(state, &id, item, *quantity);
    }
    refresh(state, &id);
    id
}

/// Adds a line for an item to a transfer. Returns its GID.
fn add_line(state: &mut ShopState, transfer: &str, item: &str, quantity: i64) -> String {
    let title = state
        .get(item)
        .and_then(|item| text(item, "variant"))
        .and_then(|variant| state.get(variant))
        .and_then(|variant| text(variant, "product"))
        .and_then(|product| state.get(product))
        .and_then(|product| product.get("title").cloned())
        .unwrap_or(Value::Null);
    let line = state.new_gid("InventoryTransferLineItem");
    state.insert(value::object(json!({
        "__typename": "InventoryTransferLineItem",
        "id": line,
        "inventoryItem": item,
        "title": title,
        "totalQuantity": quantity,
        "shippedQuantity": 0,
        "pickedForShipmentQuantity": 0,
        "processableQuantity": quantity,
        "shippableQuantity": quantity,
        TRANSFER: transfer,
        COMMITTED: 0,
    })));
    push(state, transfer, "lineItems", &line);
    line
}

/// What the shipments of a transfer hold of a line's item: `(shipped, picked)`, the units of the
/// shipments in transit or received, and those of the draft ones.
fn allocated(state: &ShopState, transfer: &str, item: &str) -> (i64, i64) {
    let mut shipped = 0;
    let mut picked = 0;
    for shipment in keys_of(state, transfer, "shipments") {
        let Some(record) = state.get(&shipment) else {
            continue;
        };
        let draft = text(record, "status") == Some("DRAFT");
        for line in gids(record.get("lineItems")) {
            let Some(line) = state.get(&line) else {
                continue;
            };
            if text(line, "inventoryItem") != Some(item) {
                continue;
            }
            if draft {
                picked += int(line, "quantity");
            } else {
                shipped += int(line, "quantity");
            }
        }
    }
    (shipped, picked)
}

/// Recomputes what a transfer derives from its lines and shipments, the way the seed and every
/// mutation do: each line's shipped, picked, processable and shippable quantities, the totals,
/// the status (in progress once a shipment is in transit, transferred once every unit shipped
/// and every shipment was received, which emits `inventory_transfers/complete`), and the units
/// committed at the origin (see [`settle`]).
pub fn refresh(state: &mut ShopState, transfer: &str) {
    let lines = keys_of(state, transfer, "lineItems");
    let mut total = 0;
    let mut left = 0;
    for line in &lines {
        let Some(item) = state
            .get(line)
            .and_then(|record| text(record, "inventoryItem"))
            .map(str::to_string)
        else {
            continue;
        };
        let (shipped, picked) = allocated(state, transfer, &item);
        if let Some(record) = state.get_mut(line) {
            let quantity = int(record, "totalQuantity");
            let processable = (quantity - shipped - picked).max(0);
            total += quantity;
            left += processable + picked;
            for (field, value) in [
                ("shippedQuantity", shipped),
                ("pickedForShipmentQuantity", picked),
                ("processableQuantity", processable),
                ("shippableQuantity", processable),
            ] {
                if record.get(field).and_then(Value::as_i64) != Some(value) {
                    record.insert(field.into(), json!(value));
                }
            }
        }
    }
    let shipments: Vec<Record> = keys_of(state, transfer, "shipments")
        .iter()
        .filter_map(|shipment| state.get(shipment).cloned())
        .collect();
    let received: i64 = shipments
        .iter()
        .map(|shipment| {
            int(shipment, "totalAcceptedQuantity") + int(shipment, "totalRejectedQuantity")
        })
        .sum();
    let sent = shipments
        .iter()
        .any(|shipment| text(shipment, "status") != Some("DRAFT"));
    let all_received = shipments
        .iter()
        .all(|shipment| text(shipment, "status") == Some("RECEIVED"));
    let Some(record) = state.get(transfer) else {
        return;
    };
    let status = text(record, "status").unwrap_or("DRAFT").to_string();
    let new = match status.as_str() {
        "CANCELED" | "TRANSFERRED" | "OTHER" => status.clone(),
        _ if sent && left == 0 && all_received && total > 0 => "TRANSFERRED".to_string(),
        _ if sent => "IN_PROGRESS".to_string(),
        other => other.to_string(),
    };
    if let Some(record) = state.get_mut(transfer) {
        record.insert("totalQuantity".into(), json!(total));
        record.insert("receivedQuantity".into(), json!(received));
        record.insert("status".into(), json!(new));
    }
    settle(state, transfer);
    if new == "TRANSFERRED" && status != "TRANSFERRED" {
        emit_change(state, "inventory_transfers/complete", transfer, json!({}));
    }
}

/// Keeps what a transfer commits at its origin right: while it is ready to ship or in progress
/// after being marked ready to ship, each line commits its units that have not shipped yet;
/// otherwise (a draft, a cancelled or transferred transfer) nothing. The difference moves between
/// `available` and `committed` ([`stock::commit`]).
fn settle(state: &mut ShopState, transfer: &str) {
    let Some(record) = state.get(transfer).cloned() else {
        return;
    };
    let (origin, _) = locations(state, &record);
    let commits = record.get(COMMITS).and_then(Value::as_bool) == Some(true)
        && matches!(
            text(&record, "status"),
            Some("READY_TO_SHIP" | "IN_PROGRESS")
        );
    for line in gids(record.get("lineItems")) {
        let Some(line_record) = state.get(&line).cloned() else {
            continue;
        };
        let current = int(&line_record, COMMITTED);
        let target = match (&origin, commits) {
            (Some(_), true) => {
                (int(&line_record, "totalQuantity") - int(&line_record, "shippedQuantity")).max(0)
            }
            _ => 0,
        };
        if target == current {
            continue;
        }
        if let (Some(origin), Some(item)) = (&origin, text(&line_record, "inventoryItem")) {
            stock::commit(state, item, origin, target - current);
        }
        if let Some(record) = state.get_mut(&line) {
            record.insert(COMMITTED.into(), json!(target));
        }
    }
}

/// `[{line_item_id, quantity}]` of lines, for `add_items` and `ready_to_ship`.
fn line_quantities(state: &ShopState, lines: &[String]) -> Value {
    Value::Array(
        lines
            .iter()
            .map(|line| {
                let quantity = state
                    .get(line)
                    .map_or(0, |record| int(record, "totalQuantity"));
                json!({"line_item_id": line, "quantity": quantity})
            })
            .collect(),
    )
}

/// Marks a transfer ready to ship: its units committed at the origin.
fn make_ready(state: &mut ShopState, transfer: &str) {
    if let Some(record) = state.get_mut(transfer) {
        record.insert("status".into(), json!("READY_TO_SHIP"));
        record.insert(COMMITS.into(), json!(true));
    }
    refresh(state, transfer);
    let lines = keys_of(state, transfer, "lineItems");
    let items = line_quantities(state, &lines);
    emit_change(
        state,
        "inventory_transfers/ready_to_ship",
        transfer,
        json!({"items": items}),
    );
}

/// A `dateCreated` input: a `DateTime`, or a `Date` (`inventoryTransferEdit`) at midnight UTC.
fn date_input(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    if let Some(date) = value::parse_date_time(text) {
        return Some(value::date_time(date));
    }
    let date = NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
    Some(format!("{date}T00:00:00Z"))
}

// --- The mutations ---

fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    create_transfer(mutation, ctx, false)
}

fn create_ready(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    create_transfer(mutation, ctx, true)
}

fn create_transfer(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    ready: bool,
) -> Result<Value, FieldError> {
    let input = super::input(ctx, "input");
    let state = mutation.state();
    let mut errors = Vec::new();
    let origin_id = text(&input, "originLocationId");
    let destination_id = text(&input, "destinationLocationId");
    let origin = check_location(
        ctx,
        state,
        origin_id,
        &["input", "originLocationId"],
        &mut errors,
    );
    let destination = check_location(
        ctx,
        state,
        destination_id,
        &["input", "destinationLocationId"],
        &mut errors,
    );
    if origin_id.is_some() && origin_id == destination_id {
        errors.push(coded(
            ctx,
            &["input", "destinationLocationId"],
            "TRANSFER_ORIGIN_CANNOT_BE_THE_SAME_AS_DESTINATION",
        ));
    }
    let lines = objects(input.get("lineItems"));
    let picked = check_lines(
        ctx,
        state,
        &lines,
        origin.as_deref(),
        &["input", "lineItems"],
        !ready,
        &mut errors,
    );
    if ready {
        if origin_id.is_none() {
            errors.push(coded(
                ctx,
                &["input", "originLocationId"],
                "LOCATION_REQUIRED",
            ));
        }
        if lines.is_empty() {
            errors.push(coded(ctx, &["input", "lineItems"], "ITEMS_EMPTY"));
        }
    }
    let tags = tags_of(&input).unwrap_or_default();
    check_tags(ctx, &tags, &mut errors);
    let metafields = check_metafields(ctx, state, None, &input, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"inventoryTransfer": null}), &errors));
    }
    let new = NewTransfer {
        origin,
        destination,
        lines: picked,
        note: input.get("note").cloned().unwrap_or(Value::Null),
        reference: input.get("referenceName").cloned().unwrap_or(Value::Null),
        tags,
        date: date_input(input.get("dateCreated")),
    };
    let state = mutation.state_mut();
    let transfer = insert_transfer(state, &new);
    set_metafields(state, &transfer, &metafields);
    announce_created(state, &transfer);
    if ready {
        make_ready(state, &transfer);
    }
    Ok(payload(json!({"inventoryTransfer": transfer}), &[]))
}

fn set_metafields(state: &mut ShopState, transfer: &str, metafields: &[Map<String, Value>]) {
    if metafields.is_empty() {
        return;
    }
    let refs: Vec<&Map<String, Value>> = metafields.iter().collect();
    let set = catalog::set_metafields(state, transfer, "TRANSFER", &refs);
    for metafield in set {
        push(state, transfer, "metafields", &metafield);
    }
}

/// The events of a transfer made: `inventory_transfers/create` (which no subscription can name)
/// and `inventory_transfers/add_items` for its lines.
fn announce_created(state: &mut ShopState, transfer: &str) {
    emit_change(state, "inventory_transfers/create", transfer, json!({}));
    let lines = keys_of(state, transfer, "lineItems");
    if !lines.is_empty() {
        let items = line_quantities(state, &lines);
        emit_change(
            state,
            "inventory_transfers/add_items",
            transfer,
            json!({"items": items}),
        );
    }
}

/// The transfer of the `id` argument, or the payload of `TRANSFER_NOT_FOUND`.
fn transfer_arg(
    ctx: &FieldContext<'_>,
    state: &ShopState,
    id: &str,
    field: &[&str],
    empty: &Value,
) -> Result<Record, Value> {
    find(state, id)
        .ok_or_else(|| payload(empty.clone(), &[coded(ctx, field, "TRANSFER_NOT_FOUND")]))
}

/// `inventoryTransferDuplicate`: a draft with the same items and quantities, locations, note,
/// reference name and tags, and no shipments.
fn duplicate(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"inventoryTransfer": null});
    let id = ctx.arg_str("id").unwrap_or_default();
    let original = match transfer_arg(ctx, mutation.state(), id, &["id"], &empty) {
        Ok(record) => record,
        Err(answer) => return Ok(answer),
    };
    let state = mutation.state();
    let (origin, destination) = locations(state, &original);
    let lines: Vec<(String, i64)> = gids(original.get("lineItems"))
        .iter()
        .filter_map(|line| {
            let record = state.get(line)?;
            Some((
                text(record, "inventoryItem")?.to_string(),
                int(record, "totalQuantity"),
            ))
        })
        .collect();
    let new = NewTransfer {
        origin,
        destination,
        lines,
        note: original.get("note").cloned().unwrap_or(Value::Null),
        reference: original
            .get("referenceName")
            .cloned()
            .unwrap_or(Value::Null),
        tags: original
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        date: None,
    };
    let state = mutation.state_mut();
    let transfer = insert_transfer(state, &new);
    announce_created(state, &transfer);
    Ok(payload(json!({"inventoryTransfer": transfer}), &[]))
}

fn edit(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"inventoryTransfer": null});
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = super::input(ctx, "input");
    let transfer = match transfer_arg(ctx, mutation.state(), &id, &["id"], &empty) {
        Ok(record) => record,
        Err(answer) => return Ok(answer),
    };
    let state = mutation.state();
    let mut errors = Vec::new();
    let draft = text(&transfer, "status") == Some("DRAFT");
    let (mut origin, mut destination) = locations(state, &transfer);
    for (field, current) in [
        ("originId", &mut origin),
        ("destinationId", &mut destination),
    ] {
        let Some(given) = input.get(field) else {
            continue;
        };
        if !draft {
            errors.push(coded(ctx, &["input", field], "TRANSFER_LOCATION_IMMUTABLE"));
            continue;
        }
        *current = match given.as_str() {
            Some(location) => {
                check_location(ctx, state, Some(location), &["input", field], &mut errors)
            }
            None => None,
        };
    }
    if origin.is_some() && origin == destination {
        errors.push(coded(
            ctx,
            &["input", "destinationId"],
            "TRANSFER_ORIGIN_CANNOT_BE_THE_SAME_AS_DESTINATION",
        ));
    }
    if draft
        && input.contains_key("originId")
        && let Some(origin) = &origin
        && items_of(state, &transfer)
            .iter()
            .any(|item| !stock::stocked(state, item, origin))
    {
        errors.push(coded(
            ctx,
            &["input", "originId"],
            "INVENTORY_STATE_NOT_ACTIVE",
        ));
    }
    let tags = tags_of(&input);
    if let Some(tags) = &tags {
        check_tags(ctx, tags, &mut errors);
    }
    let metafields = check_metafields(ctx, state, Some(&id), &input, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let origin_snapshot = input
        .contains_key("originId")
        .then(|| location_snapshot(state, origin.as_deref()));
    let destination_snapshot = input
        .contains_key("destinationId")
        .then(|| location_snapshot(state, destination.as_deref()));
    if let Some(record) = state.get_mut(&id) {
        for field in ["note", "referenceName"] {
            if let Some(value) = input.get(field) {
                record.insert(field.into(), value.clone());
            }
        }
        if let Some(tags) = tags {
            record.insert("tags".into(), json!(tags));
        }
        if let Some(date) = date_input(input.get("dateCreated")) {
            record.insert("dateCreated".into(), json!(date));
        }
        if let Some(snapshot) = origin_snapshot {
            record.insert("origin".into(), snapshot);
        }
        if let Some(snapshot) = destination_snapshot {
            record.insert("destination".into(), snapshot);
        }
    }
    set_metafields(state, &id, &metafields);
    emit_change(state, "inventory_transfers/updated", &id, json!({}));
    Ok(payload(json!({"inventoryTransfer": id}), &[]))
}

fn set_items(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"inventoryTransfer": null, "updatedLineItems": null});
    let input = super::input(ctx, "input");
    let id = text(&input, "id").unwrap_or_default().to_string();
    let transfer = match transfer_arg(ctx, mutation.state(), &id, &["input", "id"], &empty) {
        Ok(record) => record,
        Err(answer) => return Ok(answer),
    };
    let state = mutation.state();
    let status = text(&transfer, "status").unwrap_or_default();
    if !SHIPPABLE.contains(&status) {
        return Ok(payload(
            empty,
            &[coded(ctx, &["input", "id"], "INVALID_TRANSFER_STATUS")],
        ));
    }
    let (origin, _) = locations(state, &transfer);
    let mut errors = Vec::new();
    let lines = objects(input.get("lineItems"));
    let picked = check_lines(
        ctx,
        state,
        &lines,
        origin.as_deref(),
        &["input", "lineItems"],
        status == "DRAFT",
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let mut added = Vec::new();
    let mut updated = Vec::new();
    let mut answer = Vec::new();
    for (item, quantity) in &picked {
        match line_for(state, &id, item) {
            Some(line) => {
                let Some(record) = state.get(&line).cloned() else {
                    continue;
                };
                let old = int(&record, "totalQuantity");
                let kept = old - int(&record, "processableQuantity");
                let new = kept + quantity;
                if let Some(record) = state.get_mut(&line) {
                    record.insert("totalQuantity".into(), json!(new));
                }
                if new != old {
                    updated.push(
                        json!({"line_item_id": line, "old_quantity": old, "new_quantity": new}),
                    );
                }
                answer.push(json!({"inventoryItemId": item, "newQuantity": new, "deltaQuantity": new - old}));
            }
            None => {
                let line = add_line(state, &id, item, *quantity);
                added.push(json!({"line_item_id": line, "quantity": quantity}));
                answer.push(json!({"inventoryItemId": item, "newQuantity": quantity, "deltaQuantity": quantity}));
            }
        }
    }
    refresh(state, &id);
    if !added.is_empty() {
        emit_change(
            state,
            "inventory_transfers/add_items",
            &id,
            json!({"items": added}),
        );
    }
    if !updated.is_empty() {
        emit_change(
            state,
            "inventory_transfers/update_item_quantities",
            &id,
            json!({"items": updated}),
        );
    }
    Ok(payload(
        json!({"inventoryTransfer": id, "updatedLineItems": answer}),
        &[],
    ))
}

fn remove_items(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"inventoryTransfer": null, "removedQuantities": null});
    let input = super::input(ctx, "input");
    let id = text(&input, "id").unwrap_or_default().to_string();
    let transfer = match transfer_arg(ctx, mutation.state(), &id, &["input", "id"], &empty) {
        Ok(record) => record,
        Err(answer) => return Ok(answer),
    };
    let state = mutation.state();
    let status = text(&transfer, "status").unwrap_or_default();
    if !matches!(status, "DRAFT" | "READY_TO_SHIP") {
        return Ok(payload(
            empty,
            &[coded(ctx, &["input", "id"], "INVALID_TRANSFER_STATUS")],
        ));
    }
    let lines = gids(transfer.get("lineItems"));
    let asked = gids(input.get("transferLineItemIds"));
    let mut errors = Vec::new();
    for (index, line) in asked.iter().enumerate() {
        let path = ["input", "transferLineItemIds", &index.to_string()].map(str::to_string);
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        let Some(record) = state.get(line).filter(|_| lines.contains(line)) else {
            errors.push(coded(ctx, &path, "ITEM_NOT_FOUND"));
            continue;
        };
        if int(record, "processableQuantity") == 0 && int(record, "totalQuantity") > 0 {
            errors.push(coded(ctx, &path, "ALL_QUANTITY_SHIPPED"));
        }
    }
    let remaining = lines
        .iter()
        .filter(|line| {
            !asked.contains(line)
                || state.get(line).is_some_and(|record| {
                    int(record, "processableQuantity") < int(record, "totalQuantity")
                })
        })
        .count();
    if status == "READY_TO_SHIP" && remaining == 0 && !asked.is_empty() {
        errors.push(coded(
            ctx,
            &["input", "transferLineItemIds"],
            "CANT_REMOVE_ALL_ITEMS_FROM_READY_TO_SHIP_TRANSFER",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let mut removed = Vec::new();
    let mut answer = Vec::new();
    for line in &asked {
        let Some(record) = state.get(line).cloned() else {
            continue;
        };
        let total = int(&record, "totalQuantity");
        let processable = int(&record, "processableQuantity");
        let item = text(&record, "inventoryItem")
            .unwrap_or_default()
            .to_string();
        if processable == total {
            // Nothing of it is on a shipment: the line goes, and what it committed comes back.
            if let Some(record) = state.get_mut(line) {
                record.insert("totalQuantity".into(), json!(0));
            }
            settle(state, &id);
            super::pull(state, &id, "lineItems", line);
            state.remove(line);
        } else if let Some(record) = state.get_mut(line) {
            record.insert("totalQuantity".into(), json!(total - processable));
        }
        removed.push(json!({"line_item_id": line}));
        answer.push(json!({"inventoryItemId": item, "newQuantity": total - processable, "deltaQuantity": -processable}));
    }
    refresh(state, &id);
    if !removed.is_empty() {
        emit_change(
            state,
            "inventory_transfers/remove_items",
            &id,
            json!({"items": removed}),
        );
    }
    Ok(payload(
        json!({"inventoryTransfer": id, "removedQuantities": answer}),
        &[],
    ))
}

fn mark_ready(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"inventoryTransfer": null});
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let transfer = match transfer_arg(ctx, mutation.state(), &id, &["id"], &empty) {
        Ok(record) => record,
        Err(answer) => return Ok(answer),
    };
    let state = mutation.state();
    if text(&transfer, "status") != Some("DRAFT") {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "INVALID_TRANSFER_STATUS")],
        ));
    }
    let (origin, _) = locations(state, &transfer);
    let Some(origin) = origin else {
        return Ok(payload(empty, &[coded(ctx, &["id"], "LOCATION_REQUIRED")]));
    };
    if !catalog::is_active_location(state, &origin) {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "LOCATION_NOT_ACTIVE")],
        ));
    }
    if int(&transfer, "totalQuantity") == 0 {
        return Ok(payload(empty, &[coded(ctx, &["id"], "ITEMS_EMPTY")]));
    }
    if items_of(state, &transfer)
        .iter()
        .any(|item| !stock::tracked(state, item) || !stock::stocked(state, item, &origin))
    {
        return Ok(payload(empty, &[coded(ctx, &["id"], "INVALID_ITEM")]));
    }
    make_ready(mutation.state_mut(), &id);
    Ok(payload(json!({"inventoryTransfer": id}), &[]))
}

fn cancel(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"inventoryTransfer": null});
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let transfer = match transfer_arg(ctx, mutation.state(), &id, &["id"], &empty) {
        Ok(record) => record,
        Err(answer) => return Ok(answer),
    };
    if !gids(transfer.get("shipments")).is_empty() {
        return Ok(payload(empty, &[coded(ctx, &["id"], "SHIPMENT_EXISTS")]));
    }
    if !matches!(text(&transfer, "status"), Some("DRAFT" | "READY_TO_SHIP")) {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "INVALID_TRANSFER_STATUS")],
        ));
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("status".into(), json!("CANCELED"));
    }
    refresh(state, &id);
    emit_change(state, "inventory_transfers/cancel", &id, json!({}));
    Ok(payload(json!({"inventoryTransfer": id}), &[]))
}

/// `inventoryTransferDelete`: a draft, with its lines and its draft shipments.
fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"deletedId": null});
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let transfer = match transfer_arg(ctx, mutation.state(), &id, &["id"], &empty) {
        Ok(record) => record,
        Err(answer) => return Ok(answer),
    };
    if text(&transfer, "status") != Some("DRAFT") {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "INVALID_TRANSFER_STATUS")],
        ));
    }
    let state = mutation.state_mut();
    for shipment in gids(transfer.get("shipments")) {
        super::shipments::remove_shipment(state, &shipment);
    }
    state.delete(&id, "inventory_transfers/delete");
    for line in gids(transfer.get("lineItems")) {
        state.remove(&line);
    }
    for metafield in gids(transfer.get("metafields")) {
        state.remove(&metafield);
    }
    Ok(payload(json!({"deletedId": id}), &[]))
}
