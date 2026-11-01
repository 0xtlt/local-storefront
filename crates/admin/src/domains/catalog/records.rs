//! What the catalog's mutations share: reading their inputs, and making and changing the
//! records the seed makes (variants, inventory items and levels, option values, metafields, the
//! links between products and collections), in the seed's shapes, which the resolver, the
//! webhooks and the store sync read.

use serde_json::{Map, Value, json};

use super::{QUANTITY_NAMES, gids, refresh_product};
use crate::domains::content;
use crate::graphql::FieldContext;
use crate::resolve::UserError;
pub use crate::state::value::str_field as text;
pub use crate::state::value::str_field as field;
use crate::state::{Record, ShopState, gid, gid_number, parse_gid, value};

/// An argument of the field that is an input object (`{}` when it is not given).
pub fn input(ctx: &FieldContext<'_>, name: &str) -> Map<String, Value> {
    ctx.arg(name)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The objects of a list field of an input, in order.
pub fn objects<'i>(input: &'i Map<String, Value>, field: &str) -> Vec<&'i Map<String, Value>> {
    input
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .collect()
}

/// The objects of a list argument of the field.
pub fn arg_objects<'c>(ctx: &FieldContext<'c>, name: &str) -> Vec<&'c Map<String, Value>> {
    ctx.arg(name)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .collect()
}

/// The strings of a list argument of the field (the GIDs of `productIds`).
pub fn arg_strings(ctx: &FieldContext<'_>, name: &str) -> Vec<String> {
    gids(ctx.arg(name))
}

/// The currency of the shop ([`crate::domains::orders::model::currency`]).
pub fn currency(state: &ShopState) -> String {
    crate::domains::orders::model::currency(state)
}

/// The origin of the shop's URLs: `https://my-shop.myshopify.com`.
pub fn origin(state: &ShopState) -> String {
    state
        .shop()
        .and_then(|shop| field(shop, "url"))
        .unwrap_or_default()
        .to_string()
}

/// The id of the app, for its reserved metafield namespaces.
pub fn app_id(state: &ShopState) -> u64 {
    state.keys("App").next().map_or(0, gid_number)
}

/// Whether a record of `type_name` other than `except` has `handle`.
pub fn handle_taken(
    state: &ShopState,
    type_name: &str,
    handle: &str,
    except: Option<&str>,
) -> bool {
    state.keys(type_name).any(|key| {
        Some(key) != except
            && state.get(key).and_then(|record| field(record, "handle")) == Some(handle)
    })
}

/// The first record of `type_name` that has `handle`.
pub fn find_by_handle(state: &ShopState, type_name: &str, handle: &str) -> Option<String> {
    state
        .keys(type_name)
        .find(|key| state.get(key).and_then(|record| field(record, "handle")) == Some(handle))
        .map(str::to_string)
}

/// A free handle for a new record of `type_name`, made from `base` (`fallback` when that makes
/// none): itself, else with `-1`, `-2`, … appended.
pub fn free_handle(state: &ShopState, type_name: &str, base: &str, fallback: &str) -> String {
    let base = match lsf_core::util::handleize(base) {
        handle if handle.is_empty() => fallback.to_string(),
        handle => handle,
    };
    if !handle_taken(state, type_name, &base, None) {
        return base;
    }
    (1..)
        .map(|n| format!("{base}-{n}"))
        .find(|handle| !handle_taken(state, type_name, handle, None))
        .unwrap_or(base)
}

/// The inventory item of a variant.
pub fn item_of(state: &ShopState, variant: &str) -> Option<String> {
    state
        .get(variant)
        .and_then(|record| field(record, "inventoryItem"))
        .map(str::to_string)
}

/// The product of an inventory item (of its variant).
pub fn product_of_item<'s>(state: &'s ShopState, item: &str) -> Option<&'s str> {
    let variant = field(state.get(item)?, "variant")?;
    field(state.get(variant)?, "product")
}

/// Whether a location is active.
pub fn is_active_location(state: &ShopState, location: &str) -> bool {
    state
        .get(location)
        .and_then(|record| record.get("isActive"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The location a new variant is stocked at: the first active one, as Shopify stocks a new
/// variant at the shop's default location.
pub fn default_location(state: &ShopState) -> Option<String> {
    state
        .keys("Location")
        .find(|location| is_active_location(state, location))
        .map(str::to_string)
}

/// The `WeightUnit` of the shop.
pub fn weight_unit(state: &ShopState) -> String {
    match state.shop().and_then(|shop| field(shop, "weightUnit")) {
        Some(unit) if !unit.is_empty() => unit.to_string(),
        _ => "KILOGRAMS".to_string(),
    }
}

/// The GID of a new inventory level of an item: `InventoryLevel/<n>?inventory_item_id=<item>`,
/// as Shopify writes it.
fn new_level_gid(state: &mut ShopState, item: &str) -> String {
    let level = state.new_gid("InventoryLevel");
    match parse_gid(item) {
        Some((_, item_id)) => format!("{level}?inventory_item_id={item_id}"),
        None => level,
    }
}

/// The eight quantities of a new level: `available` and `on_hand` hold `available`.
pub fn new_quantities(level: &str, available: i64, now: &str) -> Vec<Value> {
    QUANTITY_NAMES
        .iter()
        .map(|name| {
            let amount = match *name {
                "available" | "on_hand" => available,
                _ => 0,
            };
            json!({
                "id": quantity_gid(level, name),
                "name": name,
                "quantity": amount,
                "updatedAt": now,
            })
        })
        .collect()
}

/// The GID of a quantity of a level, as the seed makes it.
fn quantity_gid(level: &str, name: &str) -> String {
    gid(
        "InventoryQuantity",
        lsf_core::util::stable_id("inventory_quantity", &format!("{level}/{name}")),
    )
}

/// Stocks an inventory item at a location: a new level, with `available` (and `on_hand`) at
/// `available`, listed by the item and the location. Returns its GID.
pub fn new_level(state: &mut ShopState, item: &str, location: &str, available: i64) -> String {
    let now = state.timestamp();
    let level = new_level_gid(state, item);
    state.insert(value::object(json!({
        "__typename": "InventoryLevel",
        "id": level,
        "item": item,
        "location": location,
        "quantities": new_quantities(&level, available, &now),
        "isActive": true,
        "canDeactivate": true,
        "createdAt": now,
        "updatedAt": now,
    })));
    for owner in [item, location] {
        if let Some(Value::Array(levels)) = state
            .get_mut(owner)
            .and_then(|record| record.get_mut("inventoryLevels"))
        {
            levels.push(json!(level));
        }
    }
    level
}

/// Removes a level from its item and its location, and from the shop.
pub fn remove_level(state: &mut ShopState, level: &str) {
    let Some(record) = state.get(level) else {
        return;
    };
    let owners: Vec<String> = ["item", "location"]
        .iter()
        .filter_map(|name| field(record, name))
        .map(str::to_string)
        .collect();
    for owner in owners {
        if let Some(Value::Array(levels)) = state
            .get_mut(&owner)
            .and_then(|record| record.get_mut("inventoryLevels"))
        {
            levels.retain(|known| known.as_str() != Some(level));
        }
    }
    state.remove(level);
}

/// The level of an item at a location.
pub fn level_at(state: &ShopState, item: &str, location: &str) -> Option<String> {
    let record = state.get(item)?;
    gids(record.get("inventoryLevels"))
        .into_iter()
        .find(|level| {
            state
                .get(level)
                .and_then(|record| field(record, "location"))
                == Some(location)
        })
}

/// A quantity of a level record.
pub fn quantity(level: &Record, name: &str) -> i64 {
    level
        .get("quantities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|quantity| quantity.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|quantity| quantity.get("quantity"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// The quantity names whose units are on hand: `on_hand` is their sum (shopify.dev, "Manage
/// inventory quantities and states": "The `on_hand` state equals the sum of inventory
/// quantities in the following states: available, committed, reserved, damaged, safety_stock,
/// quality_control"). `incoming` is not on hand.
pub const ON_HAND_PARTS: [&str; 6] = [
    "available",
    "committed",
    "reserved",
    "damaged",
    "safety_stock",
    "quality_control",
];

/// Changes a quantity of a level by `delta`, and `on_hand` with it when the quantity is one of
/// its parts (changing `on_hand` changes `available`, the units that are not otherwise set
/// aside). Returns the changes as `(name, delta)`, the quantity's own first, as the Admin API
/// lists them (`available` then `on_hand`).
pub fn change_quantity(
    state: &mut ShopState,
    level: &str,
    name: &str,
    delta: i64,
) -> Vec<(String, i64)> {
    let mut changes = vec![(name.to_string(), delta)];
    match name {
        "on_hand" => changes.push(("available".to_string(), delta)),
        name if ON_HAND_PARTS.contains(&name) => changes.push(("on_hand".to_string(), delta)),
        _ => {}
    }
    if delta == 0 {
        return changes;
    }
    let now = state.timestamp();
    if let Some(record) = state.get_mut(level) {
        if let Some(Value::Array(quantities)) = record.get_mut("quantities") {
            for (changed, by) in &changes {
                if let Some(quantity) = quantities.iter_mut().find(|quantity| {
                    quantity.get("name").and_then(Value::as_str) == Some(changed.as_str())
                }) {
                    let current = quantity
                        .get("quantity")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    quantity["quantity"] = json!(current + by);
                    quantity["updatedAt"] = json!(now);
                }
            }
        }
        record.insert("updatedAt".into(), json!(now));
    }
    changes
}

/// An inventory item for a variant, from an `InventoryItemInput` (or none): not tracked, ships,
/// weighs nothing in the shop's unit, unless the input says otherwise.
pub fn new_inventory_item(
    state: &mut ShopState,
    variant: &str,
    input: Option<&Map<String, Value>>,
) -> String {
    let now = state.timestamp();
    let item = state.new_gid("InventoryItem");
    let item_id = gid_number(&item);
    let mut record = value::object(json!({
        "__typename": "InventoryItem",
        "id": item,
        "sku": null,
        "tracked": false,
        "requiresShipping": true,
        "measurement": {
            "id": gid("InventoryItemMeasurement", item_id),
            "weight": {"unit": weight_unit(state), "value": 0.0},
        },
        "inventoryLevels": [],
        "variant": variant,
        "variants": [variant],
        "createdAt": now,
        "updatedAt": now,
    }));
    if let Some(input) = input {
        apply_inventory_item(&mut record, input, &currency(state));
    }
    state.insert(record);
    item
}

/// Sets the fields of an `InventoryItemInput` on an inventory item: those given, and only those.
pub fn apply_inventory_item(record: &mut Record, input: &Map<String, Value>, currency: &str) {
    if let Some(sku) = input.get("sku") {
        let sku = sku.as_str().map(str::trim).filter(|sku| !sku.is_empty());
        record.insert("sku".into(), json!(sku));
    }
    for name in ["tracked", "requiresShipping"] {
        if let Some(flag) = input.get(name).and_then(Value::as_bool) {
            record.insert(name.into(), json!(flag));
        }
    }
    if let Some(cost) = input.get("cost") {
        let cost = cost
            .as_str()
            .and_then(|amount| super::cents_of(Some(&json!(amount))))
            .map(|cents| value::money_v2(cents, currency));
        record.insert("unitCost".into(), json!(cost));
    }
    for name in [
        "countryCodeOfOrigin",
        "provinceCodeOfOrigin",
        "harmonizedSystemCode",
    ] {
        if let Some(code) = input.get(name) {
            record.insert(name.into(), code.clone());
        }
    }
    if let Some(codes) = input
        .get("countryHarmonizedSystemCodes")
        .and_then(Value::as_array)
    {
        record.insert("countryHarmonizedSystemCodes".into(), json!(codes));
    }
    if let Some(weight) = input
        .get("measurement")
        .and_then(|measurement| measurement.get("weight"))
        .and_then(Value::as_object)
    {
        let measurement = record
            .entry("measurement")
            .or_insert_with(|| json!({}))
            .as_object_mut();
        if let Some(measurement) = measurement {
            let current = measurement
                .get("weight")
                .cloned()
                .unwrap_or_else(|| json!({"unit": "KILOGRAMS", "value": 0.0}));
            let unit = weight
                .get("unit")
                .cloned()
                .unwrap_or_else(|| current.get("unit").cloned().unwrap_or(json!("KILOGRAMS")));
            let amount = weight
                .get("value")
                .and_then(Value::as_f64)
                .or_else(|| current.get("value").and_then(Value::as_f64))
                .unwrap_or(0.0);
            measurement.insert("weight".into(), json!({"unit": unit, "value": amount}));
        }
    }
}

/// Whether a variant is for sale, by Shopify's rule: its inventory is not tracked, or it has
/// stock, or it sells when out of stock (`CONTINUE`).
pub fn available_for_sale(state: &ShopState, variant: &str) -> bool {
    let Some(record) = state.get(variant) else {
        return false;
    };
    // A bundle can be sold while its components can.
    if let Some((_, sellable)) = super::bundles::bundle_inventory(state, variant) {
        return sellable;
    }
    let quantity = record
        .get("inventoryQuantity")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    !tracked(state, variant) || quantity > 0 || field(record, "inventoryPolicy") == Some("CONTINUE")
}

/// Whether a variant's inventory item is tracked.
pub fn tracked(state: &ShopState, variant: &str) -> bool {
    item_of(state, variant)
        .and_then(|item| state.get(&item))
        .and_then(|item| item.get("tracked"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The stock of the variants of products, to compare after a change: `(variant, tracked, in
/// stock)`.
pub fn stock_of(state: &ShopState, products: &[String]) -> Vec<(String, bool)> {
    products
        .iter()
        .filter_map(|product| state.get(product))
        .flat_map(|product| gids(product.get("variants")))
        .map(|variant| {
            let in_stock = in_stock(state, &variant);
            (variant, in_stock)
        })
        .collect()
}

/// Whether a tracked variant has stock (an untracked one always has; a bundle has while its
/// components make one).
fn in_stock(state: &ShopState, variant: &str) -> bool {
    let Some(record) = state.get(variant) else {
        return false;
    };
    if let Some((quantity, _)) = super::bundles::bundle_inventory(state, variant) {
        return quantity.is_none_or(|quantity| quantity > 0);
    }
    !tracked(state, variant)
        || record
            .get("inventoryQuantity")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            > 0
}

/// After a change of inventory: refreshes the products, sets each variant's `availableForSale`
/// by Shopify's rule, and emits `variants/in_stock` or `variants/out_of_stock` for the variants
/// whose stock crossed zero (shopify.dev's webhook reference: "Occurs whenever a variant
/// becomes in stock" / "out of stock").
pub fn after_inventory_change(
    state: &mut ShopState,
    products: &[String],
    before: &[(String, bool)],
) {
    for product in products {
        refresh_product(state, product);
    }
    let variants: Vec<String> = products
        .iter()
        .filter_map(|product| state.get(product))
        .flat_map(|product| gids(product.get("variants")))
        .collect();
    for variant in &variants {
        let available = available_for_sale(state, variant);
        let changed = state
            .get(variant)
            .and_then(|record| record.get("availableForSale"))
            .and_then(Value::as_bool)
            != Some(available);
        if changed && let Some(record) = state.get_mut(variant) {
            record.insert("availableForSale".into(), json!(available));
        }
    }
    for (variant, was) in before {
        if !state.contains(variant) {
            continue;
        }
        let now = in_stock(state, variant);
        if now != *was {
            let topic = if now {
                "variants/in_stock"
            } else {
                "variants/out_of_stock"
            };
            state.emit(topic, variant);
        }
    }
}

/// Checks the `metafields` input of a resource as `metafieldsSet` checks it, with the content
/// family's rules ([`content::metafields::check_owned_inputs`]): the namespace and the key (`$app`
/// and `$app:x` the app's reserved namespaces, another app's refused), the type (else the
/// definition's of `owner_type`, the namespace and the key), the value against its type and the
/// definition's validations. `owner` is the resource, `None` for one being created; an input with
/// an `id` names one of its metafields. The errors' `field` starts with `path` (`["metafields",
/// "0", "value"]`), and their `code` is fitted to `code_enum`, the code enum of the payload's
/// error type (`None` for an error type without one: `UserError`).
pub fn check_metafields(
    state: &ShopState,
    owner: Option<&str>,
    owner_type: &str,
    inputs: &[&Map<String, Value>],
    path: &[&str],
    code_enum: Option<&str>,
) -> Vec<UserError> {
    let mut errors = Vec::new();
    content::metafields::check_owned_inputs(
        state,
        owner,
        owner_type,
        Some(&as_array(inputs)),
        path,
        &mut errors,
    );
    for error in &mut errors {
        error.code =
            code_enum.and_then(|code_enum| content::fit_code(error.code.as_deref(), code_enum));
    }
    errors
}

/// Sets the metafields of a `metafields` input on an owner (a product, a variant, a collection,
/// …) of `owner_type`, normalised as `metafieldsSet` keeps them: created, or changed when the
/// owner has one of that namespace and key (or of that `id`), and linked to a definition when
/// there is one ([`content::metafields::set`]). The namespace defaults to the app's reserved one,
/// as on Shopify. The inputs are those [`check_metafields`] let through; one that does not pass
/// the checks is left out. Returns the metafields set.
pub fn set_metafields(
    state: &mut ShopState,
    owner: &str,
    owner_type: &str,
    inputs: &[&Map<String, Value>],
) -> Vec<String> {
    let mut errors = Vec::new();
    let checked = content::metafields::check_owned_inputs(
        state,
        Some(owner),
        owner_type,
        Some(&as_array(inputs)),
        &[],
        &mut errors,
    );
    checked
        .iter()
        .map(|metafield| {
            content::metafields::set(
                state,
                owner,
                &metafield.namespace,
                &metafield.key,
                &metafield.kind,
                &metafield.checked,
            )
            .0
        })
        .collect()
}

/// Inputs as the JSON array the content family's checks read.
fn as_array(inputs: &[&Map<String, Value>]) -> Value {
    Value::Array(
        inputs
            .iter()
            .map(|input| Value::Object((*input).clone()))
            .collect(),
    )
}

/// Adds the product to a collection's `products` (at the end) and the collection to the
/// product's `collections`, or takes them out of each other. Whether anything changed.
pub fn link_collection(
    state: &mut ShopState,
    collection: &str,
    product: &str,
    member: bool,
) -> bool {
    let changed = set_listed(state, collection, "products", product, member);
    // The product's own list follows, whatever the collection's said.
    set_listed(state, product, "collections", collection, member);
    changed
}

/// Whether a collection lists a product in its `products`.
pub fn in_collection(state: &ShopState, collection: &str, product: &str) -> bool {
    lists(state, collection, "products", product)
}

/// Whether an array field of `owner` lists `key`.
pub fn lists(state: &ShopState, owner: &str, field: &str, key: &str) -> bool {
    state
        .get(owner)
        .is_some_and(|record| gids(record.get(field)).iter().any(|known| known == key))
}

/// Appends `key` to (or removes it from) an array field of `owner`. Whether it was not already
/// as asked.
pub fn set_listed(
    state: &mut ShopState,
    owner: &str,
    field: &str,
    key: &str,
    member: bool,
) -> bool {
    if lists(state, owner, field, key) == member {
        return false;
    }
    if let Some(Value::Array(items)) = state
        .get_mut(owner)
        .and_then(|record| record.get_mut(field))
    {
        if member {
            items.push(json!(key));
        } else {
            items.retain(|known| known.as_str() != Some(key));
        }
    }
    true
}

/// Applies a `MoveInput` to a list of GIDs: the item `id` leaves its place and takes
/// `newPosition` (from 0), the last place when that is past the end.
pub fn apply_move(list: &mut Vec<String>, change: &Map<String, Value>) {
    let id = text(change, "id").unwrap_or_default().to_string();
    let position = change
        .get("newPosition")
        .and_then(|position| {
            position
                .as_u64()
                .or_else(|| position.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or(0);
    list.retain(|known| *known != id);
    let at = usize::try_from(position)
        .unwrap_or(usize::MAX)
        .min(list.len());
    list.insert(at, id);
}

/// A user error at a path of strings.
pub fn error_at(path: &[String], message: impl Into<String>) -> UserError {
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    UserError::new(&path, message)
}

/// The message and code of a user error that Shopify documents only by its code: the
/// description the 2026-10 schema gives the code, which is how Shopify words the messages it
/// does publish (`IDEMPOTENCY_KEY_PARAMETER_MISMATCH`: "The same idempotency key cannot be used
/// with different operation parameters.", in the schema and in shopify.dev's "Implementing
/// idempotency").
pub fn coded(field: &[&str], message: &str, code: &str) -> UserError {
    UserError::new(field, message).code(code)
}
