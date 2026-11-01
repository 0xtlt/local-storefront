//! Inventory: `inventoryAdjustQuantities`, `inventorySetQuantities`, `inventoryMoveQuantities`,
//! `inventorySetOnHandQuantities`, `inventoryActivate`, `inventoryDeactivate`,
//! `inventoryBulkToggleActivation`, `inventoryItemUpdate`, and the `@idempotent` directive the
//! inventory and location mutations require.
//!
//! As shopify.dev describes them (the reference of 2026-10, "Manage inventory quantities and
//! states", "Implementing idempotency"):
//!
//! - **Quantities**: a level has eight; `on_hand` is the sum of `available`, `committed`,
//!   `reserved`, `damaged`, `safety_stock` and `quality_control` ("The `on_hand` state equals
//!   the sum of …"); `incoming` is not on hand. Adjusting `available` (or another of them)
//!   changes `on_hand` by as much, and the answer lists both changes ("available" -4, "on_hand"
//!   -4: the reference's example); setting `on_hand` changes `available`; moving between two of
//!   them leaves `on_hand` as it is. `committed` cannot be changed through the API ("You can't use
//!   the GraphQL Admin API to adjust or move inventory quantities in the `committed` state").
//! - **Compare and swap**: each change carries `changeFromQuantity`, which must be given, `null`
//!   to skip the check ("This field is mandatory: you must explicitly pass in a value, even if
//!   that value is `null`, or the mutation returns an error"); a quantity that differs is
//!   `CHANGE_FROM_QUANTITY_STALE`.
//! - **Idempotency**: as of 2026-04 the `@idempotent(key:)` directive is required on
//!   `inventoryAdjustQuantities`, `inventorySetQuantities`, `inventoryMoveQuantities`,
//!   `inventorySetOnHandQuantities`, `inventoryActivate`, `locationActivate`,
//!   `locationDeactivate` ("calling these mutations without an idempotency directive will result
//!   in an error at runtime", Shopify's changelog). A key used again within 24 hours with the
//!   same arguments answers the first answer without doing anything again; with other arguments,
//!   `IDEMPOTENCY_KEY_PARAMETER_MISMATCH` ("The same idempotency key cannot be used with
//!   different operation parameters."). A failed attempt is not remembered (Shopify's
//!   `IDEMPOTENCY_PREVIOUS_ATTEMPT_FAILED` is not simulated): its key can be used again.
//! - **Reasons**: one of the reasons shopify.dev lists (`correction`, `cycle_count_available`,
//!   `damaged`, …), else `INVALID_REASON`. The group's `reason` is its display name: "Inventory
//!   correction" for `correction`, "Damaged" for `damaged` (the reference's examples), the reason
//!   humanized for the others *(lsf's guess)*. `quantityAfterChange` is `null`, as the Admin API
//!   answers it right after the mutation ("This field always returns `null` immediately after
//!   running the mutation").
//! - **Ledger documents**: required for every quantity name but `available`, refused for
//!   `available`, and never a `gid://shopify/` URI (the codes' descriptions).
//! - Events: `inventory_levels/update` for each level changed, `inventory_levels/connect` and
//!   `inventory_levels/disconnect` for a level created or removed, `inventory_items/update` for
//!   an item changed, `variants/in_stock` and `variants/out_of_stock` for a variant whose stock
//!   crosses zero.
//!
//! Where shopify.dev documents an error only by its code, the message is the code's description
//! in the 2026-10 schema (see [`super::records::coded`]).

use chrono::Duration;
use serde_json::{Map, Value, json};

use super::records::{
    self, ON_HAND_PARTS, arg_objects, coded, field, input, is_active_location, level_at, new_level,
    objects, quantity, remove_level, text,
};
use super::{gids, product_changed};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("inventoryAdjustQuantities", adjust_quantities)
        .mutation("inventorySetQuantities", set_quantities)
        .mutation("inventorySetOnHandQuantities", set_on_hand_quantities)
        .mutation("inventoryMoveQuantities", move_quantities)
        .mutation("inventoryActivate", activate)
        .mutation("inventoryDeactivate", deactivate)
        .mutation("inventoryBulkToggleActivation", bulk_toggle_activation)
        .mutation("inventoryItemUpdate", item_update)
        .field("InventoryAdjustmentGroup", "changes", group_changes);
}

/// The reasons an inventory change takes (shopify.dev, "Manage inventory quantities and
/// states").
const REASONS: &[&str] = &[
    "correction",
    "cycle_count_available",
    "damaged",
    "movement_created",
    "movement_updated",
    "movement_received",
    "movement_canceled",
    "other",
    "promotion",
    "quality_control",
    "received",
    "reservation_created",
    "reservation_deleted",
    "reservation_updated",
    "restock",
    "safety_stock",
    "shrinkage",
];

/// The quantity names `inventoryAdjustQuantities` changes.
const ADJUSTABLE: &[&str] = &[
    "available",
    "damaged",
    "incoming",
    "quality_control",
    "reserved",
    "safety_stock",
];

/// The quantity names `inventoryMoveQuantities` moves between.
const MOVABLE: &[&str] = &[
    "available",
    "damaged",
    "quality_control",
    "reserved",
    "safety_stock",
];

/// The display name of a reason: "Inventory correction", "Damaged".
fn reason_name(reason: &str) -> String {
    match reason {
        "correction" => "Inventory correction".to_string(),
        other => value::humanize(other),
    }
}

// --- Idempotency ---

/// What the `@idempotent` key of a mutation says to do.
pub enum Idempotency {
    /// The key answered these arguments already: the first answer, nothing done again.
    Replay(Value),
    /// Run the mutation, then [`remember`] its answer under this memo.
    Run { memo: String, fingerprint: String },
}

/// How long Shopify keeps an idempotency key ("Shopify tracks idempotency keys for 24 hours
/// from the original request").
const KEY_RETENTION_HOURS: i64 = 24;

/// Reads the `@idempotent(key:)` of a mutation that requires it. Without one, the error the
/// Admin API answers at runtime since 2026-04 (its wording is not published: lsf says what to
/// add).
pub fn idempotency(
    mutation: &Mutation<'_>,
    ctx: &FieldContext<'_>,
    empty: &Value,
    errors_key: &str,
) -> Result<Result<Idempotency, Value>, FieldError> {
    let name = ctx.field_name();
    let Some(key) = ctx
        .directive("idempotent")
        .and_then(|directive| {
            directive
                .get("key")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|key| !key.trim().is_empty())
    else {
        return Err(FieldError::new(format!(
            "{name} requires an idempotency key as of API version 2026-04: add \
             `@idempotent(key: \"<a UUID>\")` to the field \
             (https://shopify.dev/docs/api/usage/idempotent-requests)."
        )));
    };
    let fingerprint = serde_json::to_string(ctx.arguments()).unwrap_or_default();
    let memo = format!("idempotency/{name}/{key}");
    let now = mutation.now();
    if let Some(remembered) = mutation.state().memo(&memo) {
        let at = value::date_time_of(remembered.get("at"));
        let fresh = at.is_some_and(|at| now - at < Duration::hours(KEY_RETENTION_HOURS));
        if fresh {
            if remembered.get("fingerprint").and_then(Value::as_str) == Some(fingerprint.as_str()) {
                return Ok(Ok(Idempotency::Replay(
                    remembered.get("payload").cloned().unwrap_or(Value::Null),
                )));
            }
            let error = coded(
                &[],
                "The same idempotency key cannot be used with different operation parameters.",
                "IDEMPOTENCY_KEY_PARAMETER_MISMATCH",
            );
            let mut answer = empty.as_object().cloned().unwrap_or_default();
            answer.insert(errors_key.into(), json!([error.to_json()]));
            return Ok(Err(Value::Object(answer)));
        }
    }
    Ok(Ok(Idempotency::Run { memo, fingerprint }))
}

/// Remembers the answer of a mutation under its idempotency key.
pub fn remember(state: &mut ShopState, run: &Idempotency, answer: &Value) {
    if let Idempotency::Run { memo, fingerprint } = run {
        let at = state.timestamp();
        state.set_memo(
            memo.clone(),
            json!({"fingerprint": fingerprint, "payload": answer, "at": at}),
        );
    }
}

/// Runs a mutation that requires an idempotency key: replays a remembered answer, refuses a
/// key used with other arguments (the payload `empty` with the error in `errors_key`), else runs
/// `body` and remembers what it answered when it succeeded.
pub fn with_idempotency(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    empty: Value,
    errors_key: &str,
    body: fn(&mut Mutation<'_>, &FieldContext<'_>) -> Result<Value, FieldError>,
) -> Result<Value, FieldError> {
    let run = match idempotency(mutation, ctx, &empty, errors_key)? {
        Ok(Idempotency::Replay(answer)) => return Ok(answer),
        Ok(run) => run,
        Err(refused) => return Ok(refused),
    };
    let answer = body(mutation, ctx)?;
    let failed = answer
        .get(errors_key)
        .and_then(Value::as_array)
        .is_some_and(|errors| !errors.is_empty());
    if !failed {
        remember(mutation.state_mut(), &run, &answer);
    }
    Ok(answer)
}

// --- The adjustment group ---

/// One change of a group: `(item, location, name, delta, ledger document)`.
struct Change {
    item: String,
    location: String,
    name: String,
    delta: i64,
    ledger: Option<String>,
}

/// Records an `InventoryAdjustmentGroup` of changes. Returns its GID.
fn adjustment_group(
    state: &mut ShopState,
    reason: &str,
    reference: Option<&str>,
    changes: &[Change],
) -> String {
    let now = state.timestamp();
    let app = state.keys("App").next().map(str::to_string);
    let id = state.new_gid("InventoryAdjustmentGroup");
    let changes: Vec<Value> = changes
        .iter()
        .map(|change| {
            json!({
                "name": change.name,
                "delta": change.delta,
                "quantityAfterChange": null,
                "ledgerDocumentUri": change.ledger,
                "item": change.item,
                "location": change.location,
            })
        })
        .collect();
    state.insert(value::object(json!({
        "__typename": "InventoryAdjustmentGroup",
        "id": id,
        "createdAt": now,
        "reason": reason_name(reason),
        "referenceDocumentUri": reference,
        "app": app,
        "staffMember": null,
        "changes": changes,
    })));
    id
}

/// `InventoryAdjustmentGroup.changes(inventoryItemIds:, locationIds:, quantityNames:)`.
fn group_changes(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let filter = |name: &str| -> Option<Vec<String>> { ctx.arg(name).map(|ids| gids(Some(ids))) };
    let (items, locations, names) = (
        filter("inventoryItemIds"),
        filter("locationIds"),
        filter("quantityNames"),
    );
    let changes: Vec<Value> = r
        .items_of(object, "changes")
        .into_iter()
        .filter(|change| {
            let has = |list: &Option<Vec<String>>, key: &str| {
                list.as_ref().is_none_or(|list| {
                    change
                        .get(key)
                        .and_then(Value::as_str)
                        .is_some_and(|value| list.iter().any(|known| known == value))
                })
            };
            has(&items, "item") && has(&locations, "location") && has(&names, "name")
        })
        .collect();
    r.convert(ctx, &Value::Array(changes), true)
}

// --- Checks ---

/// The checks of a reason and a reference document.
fn check_reason(input: &Map<String, Value>, errors: &mut Vec<UserError>) {
    let reason = text(input, "reason").unwrap_or_default();
    if !REASONS.contains(&reason) {
        errors.push(coded(
            &["input", "reason"],
            "The specified reason is invalid.",
            "INVALID_REASON",
        ));
    }
    if let Some(reference) = text(input, "referenceDocumentUri")
        && !is_uri(reference)
    {
        errors.push(coded(
            &["input", "referenceDocumentUri"],
            "The specified reference document is invalid.",
            "INVALID_REFERENCE_DOCUMENT",
        ));
    }
}

/// Whether a text is a URI: a scheme, `:`, and something after it.
fn is_uri(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once(':') else {
        return false;
    };
    !rest.trim_start_matches('/').is_empty()
        && scheme
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The checks of a ledger document for a quantity name, under `path`.
fn check_ledger(name: &str, ledger: Option<&str>, path: &[&str], errors: &mut Vec<UserError>) {
    let mut at: Vec<&str> = path.to_vec();
    at.push("ledgerDocumentUri");
    match (name, ledger) {
        ("available", Some(_)) => errors.push(coded(
            &at,
            "A ledger document URI is not allowed when adjusting available.",
            "INVALID_AVAILABLE_DOCUMENT",
        )),
        ("available", None) => {}
        (_, None) => errors.push(coded(
            &at,
            "A ledger document URI is required except when adjusting available.",
            "INVALID_QUANTITY_DOCUMENT",
        )),
        (_, Some(uri)) if uri.starts_with("gid://shopify/") => errors.push(coded(
            &at,
            "Internal (gid://shopify/) ledger documents are not allowed to be adjusted via API.",
            "INTERNAL_LEDGER_DOCUMENT",
        )),
        (_, Some(uri)) if !is_uri(uri) => errors.push(coded(
            &at,
            "The specified ledger document is invalid.",
            "INVALID_LEDGER_DOCUMENT",
        )),
        _ => {}
    }
}

/// The level of an item at a location, or the errors of an item or a location that is not one
/// (`path` is the change's).
fn level_of(
    state: &ShopState,
    item: &str,
    location: &str,
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Option<String> {
    let at = |name: &'static str| -> Vec<&str> {
        let mut at = path.to_vec();
        at.push(name);
        at
    };
    if !state.is_of(item, "InventoryItem") {
        errors.push(coded(
            &at("inventoryItemId"),
            "The specified inventory item could not be found.",
            "INVALID_INVENTORY_ITEM",
        ));
        return None;
    }
    if !state.is_of(location, "Location") {
        errors.push(coded(
            &at("locationId"),
            "The specified location could not be found.",
            "INVALID_LOCATION",
        ));
        return None;
    }
    let level = level_at(state, item, location);
    if level.is_none() {
        // Shopify's wording for an item not stocked at a location is not published.
        errors.push(coded(
            &at("locationId"),
            "The specified inventory item is not stocked at the location.",
            "INVALID_LOCATION",
        ));
    }
    level
}

/// The check of a `changeFromQuantity`: it must be given (possibly `null`), and match.
fn check_change_from(
    input: &Map<String, Value>,
    current: i64,
    path: &[&str],
    stale_message: &str,
    errors: &mut Vec<UserError>,
) {
    let mut at: Vec<&str> = path.to_vec();
    at.push("changeFromQuantity");
    match input.get("changeFromQuantity") {
        None => errors.push(UserError::new(
            &at,
            "changeFromQuantity is required: give the quantity expected before the change, or \
             null to skip the check",
        )),
        Some(Value::Null) => {}
        Some(expected) if expected.as_i64() != Some(current) => {
            errors.push(coded(&at, stale_message, "CHANGE_FROM_QUANTITY_STALE"))
        }
        Some(_) => {}
    }
}

/// The products of the items of changes, and the bundles they are components of
/// ([`super::with_parents`]).
fn products_of(state: &ShopState, items: &[String]) -> Vec<String> {
    let mut products: Vec<String> = Vec::new();
    for item in items {
        if let Some(product) = records::product_of_item(state, item)
            && !products.iter().any(|known| known == product)
        {
            products.push(product.to_string());
        }
    }
    super::with_parents(state, products)
}

/// Runs `change` on the inventory of `items`, then refreshes their products (and the bundles
/// they are components of) and emits the events of the variants whose stock crossed zero.
fn changing_stock<T>(
    state: &mut ShopState,
    items: &[String],
    change: impl FnOnce(&mut ShopState) -> T,
) -> T {
    let products = products_of(state, items);
    let before = records::stock_of(state, &products);
    let changed = change(state);
    records::after_inventory_change(state, &products, &before);
    for product in &products {
        product_changed(state, product);
    }
    changed
}

/// Applies changes to levels: their quantities (and `on_hand` or `available` with them), the
/// derived values of their products, the events. Returns what the group lists.
fn apply(state: &mut ShopState, changes: &[(String, Change)]) -> Vec<Change> {
    let items: Vec<String> = changes
        .iter()
        .map(|(_, change)| change.item.clone())
        .collect();
    let (listed, levels) = changing_stock(state, &items, |state| {
        let mut listed: Vec<Change> = Vec::new();
        let mut levels: Vec<String> = Vec::new();
        for (level, change) in changes {
            for (name, delta) in records::change_quantity(state, level, &change.name, change.delta)
            {
                let ledger = (name == change.name)
                    .then(|| change.ledger.clone())
                    .flatten();
                match listed.iter_mut().find(|known| {
                    known.item == change.item
                        && known.location == change.location
                        && known.name == name
                }) {
                    Some(known) => known.delta += delta,
                    None => listed.push(Change {
                        item: change.item.clone(),
                        location: change.location.clone(),
                        name,
                        delta,
                        ledger,
                    }),
                }
            }
            if !levels.contains(level) {
                levels.push(level.clone());
            }
        }
        (listed, levels)
    });
    for level in &levels {
        state.emit("inventory_levels/update", level);
    }
    listed
}

// --- The mutations ---

/// `inventoryAdjustQuantities`.
fn adjust_quantities(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    with_idempotency(
        mutation,
        ctx,
        json!({"inventoryAdjustmentGroup": null}),
        "userErrors",
        adjust_quantities_once,
    )
}

fn adjust_quantities_once(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"inventoryAdjustmentGroup": null});
    let input = input(ctx, "input");
    let state = mutation.state();
    let name = text(&input, "name").unwrap_or_default().to_string();
    let mut errors = Vec::new();
    check_reason(&input, &mut errors);
    if !ADJUSTABLE.contains(&name.as_str()) {
        errors.push(coded(
            &["input", "name"],
            "The specified quantity name is invalid.",
            "INVALID_QUANTITY_NAME",
        ));
    }
    let mut changes = Vec::new();
    let mut ledgers: Vec<Option<String>> = Vec::new();
    for (index, change) in objects(&input, "changes").iter().enumerate() {
        let at = index.to_string();
        let path = ["input", "changes", at.as_str()];
        let item = text(change, "inventoryItemId")
            .unwrap_or_default()
            .to_string();
        let location = text(change, "locationId").unwrap_or_default().to_string();
        let ledger = text(change, "ledgerDocumentUri").map(str::to_string);
        if ADJUSTABLE.contains(&name.as_str()) {
            check_ledger(&name, ledger.as_deref(), &path, &mut errors);
        }
        if !ledgers.contains(&ledger) {
            ledgers.push(ledger.clone());
        }
        let delta = change.get("delta").and_then(Value::as_i64).unwrap_or(0);
        let Some(level) = level_of(state, &item, &location, &path, &mut errors) else {
            continue;
        };
        let current = state
            .get(&level)
            .map_or(0, |record| quantity(record, &name));
        check_change_from(
            change,
            current,
            &path,
            "The changeFromQuantity argument no longer matches the persisted quantity.",
            &mut errors,
        );
        let after = current + delta;
        if after > 2_000_000_000 {
            errors.push(coded(
                &[&path[..], &["delta"]].concat(),
                "The quantity can't be higher than 2,000,000,000.",
                "INVALID_QUANTITY_TOO_HIGH",
            ));
        } else if after < -2_000_000_000 {
            errors.push(coded(
                &[&path[..], &["delta"]].concat(),
                "The quantity can't be lower than -2,000,000,000.",
                "INVALID_QUANTITY_TOO_LOW",
            ));
        }
        changes.push((
            level,
            Change {
                item,
                location,
                name: name.clone(),
                delta,
                ledger,
            },
        ));
    }
    if ledgers.len() > 1 {
        errors.push(coded(
            &["input", "changes"],
            "All changes must have the same ledger document URI or, in the case of adjusting available, no ledger document URI.",
            "MAX_ONE_LEDGER_DOCUMENT",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let listed = apply(state, &changes);
    let group = adjustment_group(
        state,
        text(&input, "reason").unwrap_or_default(),
        text(&input, "referenceDocumentUri"),
        &listed,
    );
    Ok(payload(json!({"inventoryAdjustmentGroup": group}), &[]))
}

/// `inventorySetQuantities`.
fn set_quantities(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    with_idempotency(
        mutation,
        ctx,
        json!({"inventoryAdjustmentGroup": null}),
        "userErrors",
        set_quantities_once,
    )
}

fn set_quantities_once(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = input(ctx, "input");
    let name = text(&input, "name").unwrap_or_default().to_string();
    let mut errors = Vec::new();
    if !matches!(name.as_str(), "available" | "on_hand") {
        errors.push(coded(
            &["input", "name"],
            "The quantity name must be either 'available' or 'on_hand'.",
            "INVALID_NAME",
        ));
    }
    set_quantities_of(mutation, &input, "quantities", &name, errors)
}

/// `inventorySetOnHandQuantities` (deprecated for `inventorySetQuantities` with `on_hand`).
fn set_on_hand_quantities(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    with_idempotency(
        mutation,
        ctx,
        json!({"inventoryAdjustmentGroup": null}),
        "userErrors",
        |mutation, ctx| {
            let input = input(ctx, "input");
            set_quantities_of(mutation, &input, "setQuantities", "on_hand", Vec::new())
        },
    )
}

/// Sets quantities named `name` to absolute values: what `inventorySetQuantities` and
/// `inventorySetOnHandQuantities` share.
fn set_quantities_of(
    mutation: &mut Mutation<'_>,
    input: &Map<String, Value>,
    list: &str,
    name: &str,
    mut errors: Vec<UserError>,
) -> Result<Value, FieldError> {
    let empty = json!({"inventoryAdjustmentGroup": null});
    let state = mutation.state();
    check_reason(input, &mut errors);
    let mut changes = Vec::new();
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (index, quantity_input) in objects(input, list).iter().enumerate() {
        let at = index.to_string();
        let path = ["input", list, at.as_str()];
        let item = text(quantity_input, "inventoryItemId")
            .unwrap_or_default()
            .to_string();
        let location = text(quantity_input, "locationId")
            .unwrap_or_default()
            .to_string();
        if pairs.contains(&(item.clone(), location.clone())) {
            errors.push(coded(
                &path,
                "The combination of inventoryItemId and locationId must be unique.",
                "NO_DUPLICATE_INVENTORY_ITEM_ID_GROUP_ID_PAIR",
            ));
        }
        pairs.push((item.clone(), location.clone()));
        let wanted = quantity_input
            .get("quantity")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let at_quantity = [&path[..], &["quantity"]].concat();
        if wanted > 1_000_000_000 {
            errors.push(coded(
                &at_quantity,
                "The total quantity can't be higher than 1,000,000,000.",
                "INVALID_QUANTITY_TOO_HIGH",
            ));
        } else if wanted < -1_000_000_000 {
            errors.push(coded(
                &at_quantity,
                "The total quantity can't be lower than -1,000,000,000.",
                "INVALID_QUANTITY_TOO_LOW",
            ));
        } else if name == "on_hand" && wanted < 0 {
            errors.push(coded(
                &at_quantity,
                "The quantity can't be negative.",
                "INVALID_QUANTITY_NEGATIVE",
            ));
        }
        let Some(level) = level_of(state, &item, &location, &path, &mut errors) else {
            continue;
        };
        let current = state.get(&level).map_or(0, |record| quantity(record, name));
        check_change_from(
            quantity_input,
            current,
            &path,
            "The changeFromQuantity value does not match persisted value.",
            &mut errors,
        );
        changes.push((
            level,
            Change {
                item,
                location,
                name: name.to_string(),
                delta: wanted - current,
                ledger: None,
            },
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let listed = apply(state, &changes);
    let group = adjustment_group(
        state,
        text(input, "reason").unwrap_or_default(),
        text(input, "referenceDocumentUri"),
        &listed,
    );
    Ok(payload(json!({"inventoryAdjustmentGroup": group}), &[]))
}

/// `inventoryMoveQuantities`.
fn move_quantities(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    with_idempotency(
        mutation,
        ctx,
        json!({"inventoryAdjustmentGroup": null}),
        "userErrors",
        move_quantities_once,
    )
}

fn move_quantities_once(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"inventoryAdjustmentGroup": null});
    let input = input(ctx, "input");
    let state = mutation.state();
    let mut errors = Vec::new();
    check_reason(&input, &mut errors);
    let mut changes = Vec::new();
    let mut ledgers: Vec<String> = Vec::new();
    for (index, change) in objects(&input, "changes").iter().enumerate() {
        let at = index.to_string();
        let path = ["input", "changes", at.as_str()];
        let item = text(change, "inventoryItemId")
            .unwrap_or_default()
            .to_string();
        let amount = change.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if amount < 0 {
            errors.push(coded(
                &[&path[..], &["quantity"]].concat(),
                "The quantity can't be negative.",
                "INVALID_QUANTITY_NEGATIVE",
            ));
        }
        let terminal = |side: &str| {
            change
                .get(side)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        };
        let (from, to) = (terminal("from"), terminal("to"));
        let (from_location, to_location) = (
            text(&from, "locationId").unwrap_or_default().to_string(),
            text(&to, "locationId").unwrap_or_default().to_string(),
        );
        let (from_name, to_name) = (
            text(&from, "name").unwrap_or_default().to_string(),
            text(&to, "name").unwrap_or_default().to_string(),
        );
        if from_location != to_location {
            errors.push(coded(
                &path,
                "The quantities can't be moved between different locations.",
                "DIFFERENT_LOCATIONS",
            ));
        }
        if from_name == to_name {
            errors.push(coded(
                &path,
                "The quantity names for each change can't be the same.",
                "SAME_QUANTITY_NAME",
            ));
        }
        for (side, terminal, name) in [("from", &from, &from_name), ("to", &to, &to_name)] {
            let side_path = [&path[..], &[side]].concat();
            if !MOVABLE.contains(&name.as_str()) {
                errors.push(coded(
                    &[&side_path[..], &["name"]].concat(),
                    "The specified quantity name is invalid.",
                    "INVALID_QUANTITY_NAME",
                ));
                continue;
            }
            let ledger = text(terminal, "ledgerDocumentUri");
            check_ledger(name, ledger, &side_path, &mut errors);
            if let Some(ledger) = ledger
                && !ledgers.iter().any(|known| known == ledger)
            {
                ledgers.push(ledger.to_string());
            }
        }
        let Some(level) = level_of(
            state,
            &item,
            &from_location,
            &[&path[..], &["from"]].concat(),
            &mut errors,
        ) else {
            continue;
        };
        for (side, terminal, name, delta) in [
            ("from", &from, &from_name, -amount),
            ("to", &to, &to_name, amount),
        ] {
            let current = state.get(&level).map_or(0, |record| quantity(record, name));
            check_change_from(
                terminal,
                current,
                &[&path[..], &[side]].concat(),
                "The changeFromQuantity argument no longer matches the persisted quantity.",
                &mut errors,
            );
            changes.push((
                level.clone(),
                Change {
                    item: item.clone(),
                    location: from_location.clone(),
                    name: name.clone(),
                    delta,
                    ledger: text(terminal, "ledgerDocumentUri").map(str::to_string),
                },
            ));
        }
    }
    if ledgers.len() > 2 {
        errors.push(coded(
            &["input", "changes"],
            "Only a maximum of 2 ledger document URIs across all changes is allowed.",
            "MAXIMUM_LEDGER_DOCUMENT_URIS",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    // A move between two parts of `on_hand` leaves `on_hand` as it was: only the two names
    // change.
    let items: Vec<String> = changes
        .iter()
        .map(|(_, change)| change.item.clone())
        .collect();
    let (listed, levels) = changing_stock(state, &items, |state| {
        let mut listed = Vec::new();
        let mut levels: Vec<String> = Vec::new();
        for (level, change) in changes {
            set_raw(state, &level, &change.name, change.delta);
            if !levels.contains(&level) {
                levels.push(level);
            }
            listed.push(change);
        }
        (listed, levels)
    });
    for level in &levels {
        state.emit("inventory_levels/update", level);
    }
    let group = adjustment_group(
        state,
        text(&input, "reason").unwrap_or_default(),
        text(&input, "referenceDocumentUri"),
        &listed,
    );
    Ok(payload(json!({"inventoryAdjustmentGroup": group}), &[]))
}

/// Changes one quantity of a level by `delta`, alone.
fn set_raw(state: &mut ShopState, level: &str, name: &str, delta: i64) {
    let now = state.timestamp();
    if let Some(record) = state.get_mut(level) {
        if let Some(Value::Array(quantities)) = record.get_mut("quantities")
            && let Some(entry) = quantities
                .iter_mut()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        {
            let current = entry.get("quantity").and_then(Value::as_i64).unwrap_or(0);
            entry["quantity"] = json!(current + delta);
            entry["updatedAt"] = json!(now);
        }
        record.insert("updatedAt".into(), json!(now));
    }
}

/// `inventoryActivate`.
fn activate(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    with_idempotency(
        mutation,
        ctx,
        json!({"inventoryLevel": null}),
        "userErrors",
        activate_once,
    )
}

fn activate_once(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let item = ctx
        .arg_str("inventoryItemId")
        .unwrap_or_default()
        .to_string();
    let location = ctx.arg_str("locationId").unwrap_or_default().to_string();
    let mut errors = Vec::new();
    if !state.is_of(&item, "InventoryItem") {
        errors.push(UserError::new(
            &["inventoryItemId"],
            "Inventory item does not exist",
        ));
    }
    if !state.is_of(&location, "Location") {
        errors.push(UserError::new(&["locationId"], "Location does not exist"));
    } else if !is_active_location(state, &location) {
        errors.push(UserError::new(&["locationId"], "Location is not active"));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"inventoryLevel": null}), &errors));
    }
    if let Some(level) = level_at(state, &item, &location) {
        // Stocked already: the level as it is.
        return Ok(payload(json!({"inventoryLevel": level}), &[]));
    }
    // `available` sets what is for sale, `onHand` the units at the location; without them, 0.
    let available = ctx
        .arg_i64("available")
        .or_else(|| ctx.arg_i64("onHand"))
        .unwrap_or(0);
    if ctx
        .arg_i64("onHand")
        .is_some_and(|on_hand| on_hand != available)
    {
        mutation.note("inventoryActivate: lsf sets `on_hand` to `available` (the other quantities of a new level are 0)");
    }
    let state = mutation.state_mut();
    let level = changing_stock(state, std::slice::from_ref(&item), |state| {
        new_level(state, &item, &location, available)
    });
    state.emit("inventory_levels/connect", &level);
    Ok(payload(json!({"inventoryLevel": level}), &[]))
}

/// Why a level cannot be removed, if it cannot: the only level of its item, or quantities that
/// are committed, incoming or set aside.
fn cannot_deactivate(state: &ShopState, level: &str) -> Option<(&'static str, &'static str)> {
    let record = state.get(level)?;
    let item = field(record, "item")?;
    let levels = state
        .get(item)
        .map_or(0, |item| gids(item.get("inventoryLevels")).len());
    if levels <= 1 {
        return Some((
            "Cannot unstock an inventory item from the only location at which it is stocked.",
            "CANNOT_DEACTIVATE_FROM_ONLY_LOCATION",
        ));
    }
    let (committed, incoming) = (quantity(record, "committed"), quantity(record, "incoming"));
    let unavailable: i64 = ON_HAND_PARTS
        .iter()
        .filter(|name| !matches!(**name, "available" | "committed"))
        .map(|name| quantity(record, name))
        .sum();
    match (committed != 0, incoming != 0) {
        (true, true) => Some((
            "Cannot unstock this inventory item from this location because it has committed and incoming quantities.",
            "COMMITTED_AND_INCOMING_INVENTORY_AT_LOCATION",
        )),
        (true, false) => Some((
            "Cannot unstock this inventory item from this location because it has committed quantities.",
            "COMMITTED_INVENTORY_AT_LOCATION",
        )),
        (false, true) => Some((
            "Cannot unstock this inventory item from this location because it has incoming quantities.",
            "INCOMING_INVENTORY_AT_LOCATION",
        )),
        _ if unavailable != 0 => Some((
            "Cannot unstock this inventory item from this location because it has unavailable quantities.",
            "RESERVED_INVENTORY_AT_LOCATION",
        )),
        _ => None,
    }
}

/// Removes a level: emits `inventory_levels/disconnect` (with the level as it was) and
/// refreshes its product.
fn disconnect(state: &mut ShopState, level: &str) {
    let items: Vec<String> = state
        .get(level)
        .and_then(|record| field(record, "item"))
        .map(str::to_string)
        .into_iter()
        .collect();
    changing_stock(state, &items, |state| {
        state.emit("inventory_levels/disconnect", level);
        remove_level(state, level);
    });
}

/// `inventoryDeactivate`.
fn deactivate(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let level = ctx
        .arg_str("inventoryLevelId")
        .unwrap_or_default()
        .to_string();
    if !state.is_of(&level, "InventoryLevel") {
        let error = UserError::new(&["inventoryLevelId"], "Inventory level does not exist");
        return Ok(payload(json!({}), &[error]));
    }
    if let Some((message, _)) = cannot_deactivate(state, &level) {
        return Ok(payload(
            json!({}),
            &[UserError::new(&["inventoryLevelId"], message)],
        ));
    }
    disconnect(mutation.state_mut(), &level);
    Ok(payload(json!({}), &[]))
}

/// `inventoryBulkToggleActivation`.
fn bulk_toggle_activation(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"inventoryItem": null, "inventoryLevels": null});
    let state = mutation.state();
    let item = ctx
        .arg_str("inventoryItemId")
        .unwrap_or_default()
        .to_string();
    if !state.is_of(&item, "InventoryItem") {
        let error = coded(
            &["inventoryItemId"],
            "The inventory item was not found.",
            "INVENTORY_ITEM_NOT_FOUND",
        );
        return Ok(payload(empty, &[error]));
    }
    let updates = arg_objects(ctx, "inventoryItemUpdates");
    let mut errors = Vec::new();
    let mut planned = Vec::new();
    let stocked = state
        .get(&item)
        .map_or(0, |record| gids(record.get("inventoryLevels")).len());
    let mut removing = 0;
    for (index, update) in updates.iter().enumerate() {
        let at = index.to_string();
        let location = text(update, "locationId").unwrap_or_default().to_string();
        let activate = update
            .get("activate")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if !state.is_of(&location, "Location")
            || (activate && !is_active_location(state, &location))
        {
            errors.push(coded(
                &["inventoryItemUpdates", &at, "locationId"],
                "The location was not found.",
                "LOCATION_NOT_FOUND",
            ));
            continue;
        }
        let level = level_at(state, &item, &location);
        if !activate && let Some(level) = &level {
            removing += 1;
            let blocked = if removing >= stocked {
                Some((
                    "Cannot unstock an inventory item from the only location at which it is stocked.",
                    "CANNOT_DEACTIVATE_FROM_ONLY_LOCATION",
                ))
            } else {
                cannot_deactivate(state, level)
                    .filter(|(_, code)| *code != "CANNOT_DEACTIVATE_FROM_ONLY_LOCATION")
            };
            if let Some((message, code)) = blocked {
                errors.push(coded(
                    &["inventoryItemUpdates", &at, "locationId"],
                    message,
                    code,
                ));
            }
        }
        planned.push((location, activate, level));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let mut activated = Vec::new();
    for (location, activate, level) in planned {
        match (activate, level) {
            (true, None) => {
                let level = new_level(state, &item, &location, 0);
                state.emit("inventory_levels/connect", &level);
                activated.push(level);
            }
            (true, Some(level)) => activated.push(level),
            (false, Some(level)) => disconnect(state, &level),
            (false, None) => {}
        }
    }
    for product in products_of(state, std::slice::from_ref(&item)) {
        product_changed(state, &product);
    }
    Ok(payload(
        json!({"inventoryItem": item, "inventoryLevels": activated}),
        &[],
    ))
}

/// `inventoryItemUpdate`.
fn item_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let item = ctx.arg_str("id").unwrap_or_default().to_string();
    if !state.is_of(&item, "InventoryItem") {
        let error = UserError::new(&["id"], "Inventory item does not exist");
        return Ok(payload(json!({"inventoryItem": null}), &[error]));
    }
    let input = input(ctx, "input");
    let mut errors = Vec::new();
    if let Some(cost) = input.get("cost").filter(|cost| !cost.is_null())
        && super::variants::money_cents(cost).is_none_or(|cents| cents < 0)
    {
        errors.push(UserError::new(
            &["input", "cost"],
            "Cost must be greater than or equal to 0",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"inventoryItem": null}), &errors));
    }
    if input
        .get("measurement")
        .and_then(|measurement| measurement.get("shippingPackageId"))
        .is_some()
    {
        mutation.note("inventoryItemUpdate: lsf does not simulate shipping packages: `shippingPackageId` was ignored");
    }
    let state = mutation.state_mut();
    let currency = records::currency(state);
    if let Some(record) = state.get_mut(&item) {
        records::apply_inventory_item(record, &input, &currency);
    }
    let variants: Vec<String> = state
        .get(&item)
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default();
    let sku = state
        .get(&item)
        .and_then(|record| record.get("sku").cloned());
    for variant in &variants {
        if let Some(sku) = &sku
            && let Some(record) = state.get_mut(variant)
        {
            record.insert("sku".into(), sku.clone());
        }
    }
    changing_stock(state, std::slice::from_ref(&item), |_| {});
    state.touch(&item);
    state.emit("inventory_items/update", &item);
    Ok(payload(json!({"inventoryItem": item}), &[]))
}
