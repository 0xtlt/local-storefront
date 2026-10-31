//! The variants of a product: `productVariantsBulkCreate`, `productVariantsBulkUpdate`,
//! `productVariantsBulkDelete`, `productVariantsBulkReorder`, and the variant records the other
//! product mutations make (`productCreate`, `productSet`, `productDuplicate`).
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A variant names one value of each option of its product (`optionValues`: by `optionId` or
//!   `optionName`, the value by `id` or `name`). A value name the option does not have is added
//!   to it (the reference's example creates "Golden" on the option "Title"). Two variants with
//!   the same values are refused: "The variant 'Aqua Blue / S' already exists." (a response
//!   quoted on community.shopify.dev).
//! - `productVariantsBulkCreate`'s `strategy`: `DEFAULT` deletes the standalone "Default Title"
//!   variant when it is the product's only one and keeps a standalone custom one;
//!   `REMOVE_STANDALONE_VARIANT` deletes the only variant, whichever it is;
//!   `PRESERVE_STANDALONE_VARIANT` keeps it (the enum's descriptions).
//! - A new variant costs `0.00` unless `price` says otherwise, sells while out of stock only with
//!   `inventoryPolicy: CONTINUE` (`DENY` by default), and its inventory item is not tracked
//!   unless `inventoryItem.tracked` says so (as `productCreate`'s example response). It is
//!   stocked at the locations of `inventoryQuantities` with their `availableQuantity`, else at
//!   the shop's first active location with 0.
//! - `productVariantsBulkUpdate` changes the fields given; `inventoryQuantities` are refused
//!   ("Inventory quantities can only be provided during create. To update inventory for
//!   existing variants, use inventoryAdjustQuantities.", the code's description), and
//!   `quantityAdjustments` change the `available` quantity at a location. Without
//!   `allowPartialUpdates`, one error changes nothing; with it, the valid variants are updated
//!   and the errors reported.
//! - `productVariantsBulkDelete` refuses to remove every variant ("Cannot delete default
//!   variant.", the code's description) and a variant of another product ("At least one variant
//!   does not belong to the product", `field` `["variantsIds", "1"]`, the reference's example).
//! - Events: `inventory_items/create` and `inventory_levels/connect` for each new variant,
//!   `inventory_items/update` for an inventory item changed, `inventory_levels/update` for a
//!   quantity adjusted, `inventory_items/delete` for a variant removed, then `products/update`
//!   (the product payload lists the variants).
//!
//! Where shopify.dev documents an error only by its code, the message is the code's description
//! in the 2026-10 schema (see [`super::records::coded`]).

use serde_json::{Map, Value, json};

use super::media;
use super::records::{
    self, apply_inventory_item, arg_objects, coded, currency, default_location, field, is,
    is_active_location, item_of, new_inventory_item, new_level, objects, text,
};
use super::{cents_of, gids, product_changed};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("productVariantsBulkCreate", bulk_create)
        .mutation("productVariantsBulkUpdate", bulk_update)
        .mutation("productVariantsBulkDelete", bulk_delete)
        .mutation("productVariantsBulkReorder", bulk_reorder);
}

/// The most variants a product has by default (shopify.dev: "By default, stores have a limit of
/// 2048 product variants for each product").
pub const VARIANT_LIMIT: usize = 2048;

/// An option of a product, with its values: `(GID, name)`.
#[derive(Clone, Debug)]
pub struct ProductOption {
    pub id: String,
    pub name: String,
    pub values: Vec<(String, String)>,
}

/// The options of a product, in their order.
pub fn options_of(state: &ShopState, product: &str) -> Vec<ProductOption> {
    let ids = state
        .get(product)
        .map(|record| gids(record.get("options")))
        .unwrap_or_default();
    let mut options: Vec<(i64, ProductOption)> = ids
        .iter()
        .filter_map(|id| {
            let record = state.get(id)?;
            let values = gids(record.get("optionValues"))
                .into_iter()
                .filter_map(|value| {
                    let name = field(state.get(&value)?, "name")?.to_string();
                    Some((value, name))
                })
                .collect();
            Some((
                record.get("position").and_then(Value::as_i64).unwrap_or(0),
                ProductOption {
                    id: id.clone(),
                    name: field(record, "name").unwrap_or_default().to_string(),
                    values,
                },
            ))
        })
        .collect();
    options.sort_by_key(|(position, _)| *position);
    options.into_iter().map(|(_, option)| option).collect()
}

/// The value a variant selects for an option: the option's GID, the value's name, and the
/// value's GID when it exists already.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub option: String,
    pub name: String,
    pub value: Option<String>,
}

/// Which mutation reads option values: the codes of its errors differ.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    BulkCreate,
    BulkUpdate,
    Set,
}

/// Reads the `optionValues` of a variant input against the options of the product: one choice
/// per option, in the options' order. The errors are under `path`.
pub fn choices(
    options: &[ProductOption],
    values: &[&Map<String, Value>],
    path: &[String],
    reader: Reader,
) -> Result<Vec<Choice>, Vec<UserError>> {
    let mut errors = Vec::new();
    let error = |suffix: &[&str], message: &str, code: &str| UserError {
        field: Some(
            path.iter()
                .cloned()
                .chain(suffix.iter().map(|part| part.to_string()))
                .collect(),
        ),
        message: message.to_string(),
        code: Some(code.to_string()),
    };
    let mut chosen: Vec<Option<Choice>> = vec![None; options.len()];
    for (index, input) in values.iter().enumerate() {
        let position = index.to_string();
        let option = match (text(input, "optionId"), text(input, "optionName")) {
            (Some(id), _) => options.iter().position(|option| option.id == id),
            (None, Some(name)) => options
                .iter()
                .position(|option| option.name == name)
                .or_else(|| {
                    options
                        .iter()
                        .position(|option| option.name.eq_ignore_ascii_case(name))
                }),
            (None, None) => None,
        };
        let Some(option) = option else {
            let code = match reader {
                Reader::BulkCreate => "OPTION_VALUES_FOR_NUMBER_OF_UNKNOWN_OPTIONS",
                Reader::BulkUpdate | Reader::Set => "OPTION_DOES_NOT_EXIST",
            };
            let message = match reader {
                Reader::BulkCreate => "Variant options are more than the product options.",
                Reader::BulkUpdate | Reader::Set => "Option does not exist.",
            };
            errors.push(error(&["optionValues", &position], message, code));
            continue;
        };
        let known = &options[option].values;
        let choice = match (text(input, "id"), text(input, "name")) {
            (Some(id), _) => match known.iter().find(|(value, _)| value == id) {
                Some((value, name)) => Choice {
                    option: options[option].id.clone(),
                    name: name.clone(),
                    value: Some(value.clone()),
                },
                None => {
                    let code = match reader {
                        Reader::BulkCreate => "INVALID_INPUT",
                        Reader::BulkUpdate | Reader::Set => "OPTION_VALUE_DOES_NOT_EXIST",
                    };
                    errors.push(error(
                        &["optionValues", &position, "id"],
                        "Option value does not exist.",
                        code,
                    ));
                    continue;
                }
            },
            (None, Some(name)) if !name.trim().is_empty() => {
                let name = name.trim();
                let value = known.iter().find(|(_, known)| known == name).or_else(|| {
                    known
                        .iter()
                        .find(|(_, known)| known.eq_ignore_ascii_case(name))
                });
                Choice {
                    option: options[option].id.clone(),
                    name: value.map_or(name, |(_, known)| known.as_str()).to_string(),
                    value: value.map(|(id, _)| id.clone()),
                }
            }
            _ => {
                errors.push(error(
                    &["optionValues", &position, "name"],
                    "Option value name can't be blank",
                    "INVALID_INPUT",
                ));
                continue;
            }
        };
        if chosen[option].is_some() {
            errors.push(error(
                &["optionValues", &position],
                "Variant options are more than the product options.",
                match reader {
                    Reader::BulkCreate | Reader::BulkUpdate => {
                        "OPTION_VALUES_FOR_NUMBER_OF_UNKNOWN_OPTIONS"
                    }
                    Reader::Set => "INVALID_VARIANT",
                },
            ));
            continue;
        }
        chosen[option] = Some(choice);
    }
    if errors.is_empty() && chosen.iter().any(Option::is_none) {
        let (message, code) = match reader {
            Reader::Set => (
                "Option values are missing for some options.",
                "OPTION_VALUES_MISSING",
            ),
            _ => (
                "Variant options are not enough.",
                "NEED_TO_ADD_OPTION_VALUES",
            ),
        };
        errors.push(error(&["optionValues"], message, code));
    }
    if errors.is_empty() {
        Ok(chosen.into_iter().flatten().collect())
    } else {
        Err(errors)
    }
}

/// The title of the variant of these choices: the values, `/`-separated.
pub fn title_of(choices: &[Choice]) -> String {
    choices
        .iter()
        .map(|choice| choice.name.as_str())
        .collect::<Vec<_>>()
        .join(" / ")
}

/// The values a variant selects, in lowercase, to find duplicates.
pub fn combination(choices: &[Choice]) -> Vec<String> {
    choices
        .iter()
        .map(|choice| choice.name.to_lowercase())
        .collect()
}

/// The combination of an existing variant.
pub fn combination_of(state: &ShopState, variant: &str) -> Vec<String> {
    state
        .get(variant)
        .and_then(|record| record.get("selectedOptions"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|option| option.get("value").and_then(Value::as_str))
        .map(str::to_lowercase)
        .collect()
}

/// Checks the plain fields of a variant input (prices, barcodes): errors under `path`.
pub fn check_fields(input: &Map<String, Value>, path: &[String], errors: &mut Vec<UserError>) {
    let at = |name: &str| -> Option<Vec<String>> {
        Some(path.iter().cloned().chain([name.to_string()]).collect())
    };
    for name in ["price", "compareAtPrice"] {
        let Some(given) = input.get(name).filter(|value| !value.is_null()) else {
            continue;
        };
        match money_cents(given) {
            None => errors.push(UserError {
                field: at(name),
                message: format!("{} is invalid", value::humanize(name)),
                code: Some("INVALID_INPUT".into()),
            }),
            // Rails' numericality validation, as Shopify words it for a variant's price.
            Some(cents) if cents < 0 => errors.push(UserError {
                field: at(name),
                message: format!(
                    "{} must be greater than or equal to 0",
                    value::humanize(name)
                ),
                code: Some("GREATER_THAN_OR_EQUAL_TO".into()),
            }),
            Some(_) => {}
        }
    }
}

/// The cents of a `Money` input, written as a string or a number.
pub fn money_cents(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => cents_of(Some(&json!(number.to_string()))),
        Value::String(_) => cents_of(Some(value)),
        _ => None,
    }
}

/// The `Money` of a price input: `"19.99"`.
fn money_text(value: &Value) -> Option<String> {
    money_cents(value).map(value::money)
}

/// Makes sure each choice's value exists on its option: a name the option does not have is
/// added to it (a new `ProductOptionValue`). Returns the choices with their value GIDs.
pub fn ensure_values(state: &mut ShopState, choices: &[Choice]) -> Vec<Choice> {
    choices
        .iter()
        .map(|choice| {
            if choice.value.is_some() {
                return choice.clone();
            }
            let existing = state.get(&choice.option).and_then(|option| {
                gids(option.get("optionValues")).into_iter().find(|value| {
                    state.get(value).and_then(|record| field(record, "name"))
                        == Some(choice.name.as_str())
                })
            });
            let value = existing.unwrap_or_else(|| add_value(state, &choice.option, &choice.name));
            Choice {
                value: Some(value),
                ..choice.clone()
            }
        })
        .collect()
}

/// Adds a value to an option. Returns its GID.
pub fn add_value(state: &mut ShopState, option: &str, name: &str) -> String {
    let id = state.new_gid("ProductOptionValue");
    state.insert(value::object(json!({
        "__typename": "ProductOptionValue",
        "id": id,
        "name": name,
        "hasVariants": false,
        "swatch": null,
    })));
    if let Some(record) = state.get_mut(option) {
        if let Some(Value::Array(values)) = record.get_mut("optionValues") {
            values.push(json!(id));
        }
        if let Some(Value::Array(values)) = record.get_mut("values") {
            values.push(json!(name));
        }
    }
    id
}

/// How a new variant is stocked: `(location, available)`.
pub type Stock = Vec<(String, i64)>;

/// Creates a variant of a product, with its inventory item and levels, at the end of its
/// variants: the choices (whose values must exist, see [`ensure_values`]), the fields of the
/// input, the stock. Returns the variant's GID; the product is not refreshed.
pub fn insert_variant(
    state: &mut ShopState,
    product: &str,
    choices: &[Choice],
    input: &Map<String, Value>,
    stock: &Stock,
) -> String {
    let now = state.timestamp();
    let product_title = state
        .get(product)
        .and_then(|record| field(record, "title"))
        .unwrap_or_default()
        .to_string();
    let option_names: Vec<(String, String)> = choices
        .iter()
        .map(|choice| {
            let name = state
                .get(&choice.option)
                .and_then(|record| field(record, "name"))
                .unwrap_or_default()
                .to_string();
            (name, choice.name.clone())
        })
        .collect();
    let title = title_of(choices);
    let variant = state.new_gid("ProductVariant");
    let item = new_inventory_item(
        state,
        &variant,
        input.get("inventoryItem").and_then(Value::as_object),
    );
    let selected: Vec<Value> = option_names
        .iter()
        .zip(choices)
        .map(|((name, value), choice)| {
            json!({"name": name, "value": value, "optionValue": choice.value})
        })
        .collect();
    let position = state
        .get(product)
        .map_or(0, |record| gids(record.get("variants")).len())
        + 1;
    state.insert(value::object(json!({
        "__typename": "ProductVariant",
        "id": variant,
        "title": title,
        "displayName": format!("{product_title} - {title}"),
        "sku": null,
        "barcode": null,
        "barcodes": [],
        "price": "0.00",
        "compareAtPrice": null,
        "position": position,
        "selectedOptions": selected,
        "inventoryItem": item,
        "inventoryPolicy": "DENY",
        "inventoryQuantity": 0,
        "availableForSale": true,
        "taxable": true,
        "requiresComponents": false,
        "product": product,
        "media": [],
        "image": null,
        "unitPrice": null,
        "showUnitPrice": false,
        "sellingPlanGroups": [],
        "createdAt": now,
        "updatedAt": now,
        "metafields": [],
    })));
    if let Some(Value::Array(variants)) = state
        .get_mut(product)
        .and_then(|record| record.get_mut("variants"))
    {
        variants.push(json!(variant));
    }
    apply_fields(state, &variant, input);
    let sku = state
        .get(&item)
        .and_then(|record| record.get("sku").cloned())
        .unwrap_or(Value::Null);
    if let Some(record) = state.get_mut(&variant) {
        record.insert("sku".into(), sku);
    }
    for (location, available) in stock {
        new_level(state, &item, location, *available);
    }
    let attached = media::media_of_variant_input(state, product, input);
    media::attach_to_variant(state, &variant, &attached);
    let metafields = objects(input, "metafields");
    records::set_metafields(state, &variant, "PRODUCTVARIANT", &metafields);
    variant
}

/// Sets the plain fields of a variant input on a variant (and its inventory item): those given,
/// and only those. Whether the inventory item changed.
pub fn apply_fields(state: &mut ShopState, variant: &str, input: &Map<String, Value>) -> bool {
    let currency = currency(state);
    let item = item_of(state, variant);
    let mut item_changed = false;
    if let Some(item) = &item {
        let mut item_input = input
            .get("inventoryItem")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        // `productSet` gives the SKU on the variant.
        if let Some(sku) = input.get("sku") {
            item_input.insert("sku".into(), sku.clone());
        }
        if !item_input.is_empty()
            && let Some(record) = state.get_mut(item)
        {
            let before = record.clone();
            apply_inventory_item(record, &item_input, &currency);
            item_changed = *record != before;
        }
    }
    let sku = item
        .as_ref()
        .and_then(|item| state.get(item))
        .and_then(|record| record.get("sku").cloned());
    let Some(record) = state.get_mut(variant) else {
        return item_changed;
    };
    if let Some(sku) = sku {
        record.insert("sku".into(), sku);
    }
    if let Some(price) = input.get("price").and_then(money_text) {
        record.insert("price".into(), json!(price));
    }
    if let Some(price) = input.get("compareAtPrice") {
        record.insert("compareAtPrice".into(), json!(money_text(price)));
    }
    let barcode = match (input.get("barcodes"), input.get("barcode")) {
        (Some(Value::Array(barcodes)), _) => Some(
            barcodes
                .iter()
                .filter_map(|barcode| barcode.get("value").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>(),
        ),
        (_, Some(barcode)) => Some(
            barcode
                .as_str()
                .filter(|barcode| !barcode.is_empty())
                .map(str::to_string)
                .into_iter()
                .collect(),
        ),
        _ => None,
    };
    if let Some(barcodes) = barcode {
        record.insert("barcode".into(), json!(barcodes.first()));
        let list: Vec<Value> = barcodes
            .iter()
            .map(|barcode| json!({"value": barcode}))
            .collect();
        record.insert("barcodes".into(), json!(list));
    }
    if let Some(policy) = text(input, "inventoryPolicy") {
        record.insert("inventoryPolicy".into(), json!(policy));
    }
    for name in ["taxable", "requiresComponents", "showUnitPrice"] {
        if let Some(flag) = input.get(name).and_then(Value::as_bool) {
            record.insert(name.into(), json!(flag));
        }
    }
    if let Some(code) = input.get("taxCode") {
        record.insert("taxCode".into(), code.clone());
    }
    if let Some(measurement) = input.get("unitPriceMeasurement") {
        let measurement = measurement.as_object().map(|measurement| {
            json!({
                "measuredType": measured_type(measurement.get("quantityUnit").and_then(Value::as_str)),
                "quantityValue": measurement.get("quantityValue"),
                "quantityUnit": measurement.get("quantityUnit"),
                "referenceValue": measurement.get("referenceValue"),
                "referenceUnit": measurement.get("referenceUnit"),
            })
        });
        record.insert("unitPriceMeasurement".into(), json!(measurement));
    }
    item_changed
}

/// The `UnitPriceMeasurementMeasuredType` of a unit.
fn measured_type(unit: Option<&str>) -> &'static str {
    match unit.unwrap_or_default() {
        "MG" | "G" | "KG" | "OZ" | "LB" => "WEIGHT",
        "ML" | "CL" | "L" | "M3" | "FLOZ" | "PT" | "QT" | "GAL" => "VOLUME",
        "MM" | "CM" | "M" | "IN" | "FT" | "YD" => "LENGTH",
        "M2" | "FT2" => "AREA",
        "ITEM" => "COUNT",
        _ => "UNKNOWN",
    }
}

/// Changes the option values a variant selects.
pub fn set_choices(state: &mut ShopState, variant: &str, choices: &[Choice]) {
    let names: Vec<String> = choices
        .iter()
        .map(|choice| {
            state
                .get(&choice.option)
                .and_then(|record| field(record, "name"))
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let selected: Vec<Value> = names
        .iter()
        .zip(choices)
        .map(|(name, choice)| json!({"name": name, "value": choice.name, "optionValue": choice.value}))
        .collect();
    if let Some(record) = state.get_mut(variant) {
        record.insert("selectedOptions".into(), json!(selected));
        record.insert("title".into(), json!(title_of(choices)));
    }
}

/// Removes a variant of a product, with its inventory item and levels: emits
/// `inventory_items/delete`.
pub fn remove_variant(state: &mut ShopState, product: &str, variant: &str) {
    if let Some(item) = item_of(state, variant) {
        let levels = state
            .get(&item)
            .map(|record| gids(record.get("inventoryLevels")))
            .unwrap_or_default();
        for level in levels {
            records::remove_level(state, &level);
        }
        state.delete(&item, "inventory_items/delete");
    }
    let metafields = state
        .get(variant)
        .map(|record| gids(record.get("metafields")))
        .unwrap_or_default();
    for metafield in metafields {
        crate::domains::content::metafields::remove(state, &metafield);
    }
    for group in state
        .keys("SellingPlanGroup")
        .map(str::to_string)
        .collect::<Vec<_>>()
    {
        if let Some(Value::Array(variants)) = state
            .get_mut(&group)
            .and_then(|record| record.get_mut("productVariants"))
        {
            variants.retain(|known| known.as_str() != Some(variant));
        }
    }
    state.remove(variant);
    if let Some(Value::Array(variants)) = state
        .get_mut(product)
        .and_then(|record| record.get_mut("variants"))
    {
        variants.retain(|known| known.as_str() != Some(variant));
    }
}

/// The stock of a `productVariantsBulkCreate` variant: its `inventoryQuantities`, else the
/// first active location with 0. Errors for a location that is not one.
fn bulk_stock(
    state: &ShopState,
    input: &Map<String, Value>,
    path: &[String],
    errors: &mut Vec<UserError>,
) -> Stock {
    let quantities = objects(input, "inventoryQuantities");
    if quantities.is_empty() {
        return default_location(state)
            .map(|location| vec![(location, 0)])
            .unwrap_or_default();
    }
    let mut stock = Vec::new();
    for (index, quantity) in quantities.iter().enumerate() {
        let location = text(quantity, "locationId").unwrap_or_default();
        if !is(state, location, "Location") || !is_active_location(state, location) {
            errors.push(UserError {
                field: Some(
                    path.iter()
                        .cloned()
                        .chain([
                            "inventoryQuantities".to_string(),
                            index.to_string(),
                            "locationId".to_string(),
                        ])
                        .collect(),
                ),
                message: "Quantity could not be set. The location was not found.".into(),
                code: Some("TRACKED_VARIANT_LOCATION_NOT_FOUND".into()),
            });
            continue;
        }
        let available = quantity
            .get("availableQuantity")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        stock.push((location.to_string(), available));
    }
    stock
}

/// The product of `productId`.
fn product_arg(state: &ShopState, ctx: &FieldContext<'_>) -> Option<String> {
    ctx.arg_str("productId")
        .filter(|id| is(state, id, "Product"))
        .map(str::to_string)
}

fn no_product() -> UserError {
    coded(
        &["productId"],
        "Product does not exist",
        "PRODUCT_DOES_NOT_EXIST",
    )
}

fn path_of(index: usize) -> Vec<String> {
    vec!["variants".to_string(), index.to_string()]
}

/// `productVariantsBulkCreate`.
fn bulk_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"product": null, "productVariants": null});
    let state = mutation.state();
    let Some(product) = product_arg(state, ctx) else {
        return Ok(payload(empty, &[no_product()]));
    };
    let strategy = ctx.arg_str("strategy").unwrap_or("DEFAULT");
    let inputs = arg_objects(ctx, "variants");
    let options = options_of(state, &product);
    let existing = state
        .get(&product)
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default();
    let standalone = (existing.len() == 1).then(|| existing[0].clone());
    let standalone_is_default = standalone.as_ref().is_some_and(|variant| {
        state.get(variant).and_then(|record| field(record, "title")) == Some("Default Title")
    });
    let removed = match (strategy, &standalone) {
        ("REMOVE_STANDALONE_VARIANT", Some(variant)) => Some(variant.clone()),
        ("DEFAULT", Some(variant)) if standalone_is_default => Some(variant.clone()),
        _ => None,
    };
    let mut taken: Vec<Vec<String>> = existing
        .iter()
        .filter(|variant| Some(*variant) != removed.as_ref())
        .map(|variant| combination_of(state, variant))
        .collect();

    let (media_errors, media_ok) = media::check_media(&media::media_arg(ctx), &["media"]);
    let mut errors = media_errors;
    let mut planned = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let path = path_of(index);
        if input.contains_key("id") {
            errors.push(UserError {
                field: Some(path.iter().cloned().chain(["id".to_string()]).collect()),
                message: "On create, this key cannot be used.".into(),
                code: Some("NO_KEY_ON_CREATE".into()),
            });
        }
        check_fields(input, &path, &mut errors);
        let stock = bulk_stock(state, input, &path, &mut errors);
        let values = objects(input, "optionValues");
        match choices(&options, &values, &path, Reader::BulkCreate) {
            Ok(choices) => {
                let key = combination(&choices);
                if taken.contains(&key) {
                    errors.push(UserError {
                        field: Some(path.clone()),
                        message: format!("The variant '{}' already exists.", title_of(&choices)),
                        code: Some("VARIANT_ALREADY_EXISTS".into()),
                    });
                } else {
                    taken.push(key);
                }
                planned.push((choices, *input, stock));
            }
            Err(found) => errors.extend(found),
        }
    }
    if taken.len() > VARIANT_LIMIT {
        errors.push(coded(
            &["variants"],
            "Variant limit reached. A product can have at most 2048 variants.",
            "INVALID_INPUT",
        ));
    }
    if !errors.is_empty() || media_ok.contains(&false) {
        return Ok(payload(empty, &errors));
    }
    if inputs
        .iter()
        .any(|input| input.contains_key("quantityAdjustments"))
    {
        mutation.note(
            "productVariantsBulkCreate: `quantityAdjustments` is for updates (use `inventoryQuantities` to stock a new variant): it was ignored",
        );
    }

    let state = mutation.state_mut();
    for input in media::media_arg(ctx) {
        media::create_media(state, &product, input);
    }
    let mut created = Vec::new();
    let mut events: Vec<(&str, String)> = Vec::new();
    for (choices, input, stock) in &planned {
        let choices = ensure_values(state, choices);
        let variant = insert_variant(state, &product, &choices, input, stock);
        if let Some(item) = item_of(state, &variant) {
            events.push(("inventory_items/create", item.clone()));
            for level in state
                .get(&item)
                .map(|record| gids(record.get("inventoryLevels")))
                .unwrap_or_default()
            {
                events.push(("inventory_levels/connect", level));
            }
        }
        created.push(variant);
    }
    if let Some(variant) = &removed {
        remove_variant(state, &product, variant);
    }
    product_changed(state, &product);
    state.touch(&product);
    for (topic, gid) in &events {
        state.emit(topic, gid);
    }
    state.emit("products/update", &product);
    Ok(payload(
        json!({"product": product, "productVariants": created}),
        &[],
    ))
}

/// `productVariantsBulkUpdate`.
fn bulk_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"product": null, "productVariants": null});
    let state = mutation.state();
    let Some(product) = product_arg(state, ctx) else {
        return Ok(payload(empty, &[no_product()]));
    };
    let partial = ctx.arg_bool("allowPartialUpdates").unwrap_or(false);
    let inputs = arg_objects(ctx, "variants");
    let options = options_of(state, &product);
    let own = state
        .get(&product)
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default();
    let (media_errors, media_ok) = media::check_media(&media::media_arg(ctx), &["media"]);
    let mut errors = media_errors;
    // What each variant becomes: its combination, to find duplicates.
    let mut combinations: Vec<(String, Vec<String>)> = own
        .iter()
        .map(|variant| (variant.clone(), combination_of(state, variant)))
        .collect();
    let mut planned = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let path = path_of(index);
        let mut own_errors = Vec::new();
        let id = text(input, "id").unwrap_or_default().to_string();
        if id.is_empty() {
            own_errors.push(UserError {
                field: Some(path.iter().cloned().chain(["id".to_string()]).collect()),
                message: "Product variant is missing ID attribute.".into(),
                code: Some("PRODUCT_VARIANT_ID_MISSING".into()),
            });
        } else if !is(state, &id, "ProductVariant") {
            own_errors.push(UserError {
                field: Some(path.iter().cloned().chain(["id".to_string()]).collect()),
                message: "Product variant does not exist".into(),
                code: Some("PRODUCT_VARIANT_DOES_NOT_EXIST".into()),
            });
        } else if !own.contains(&id) {
            own_errors.push(UserError {
                field: Some(path.iter().cloned().chain(["id".to_string()]).collect()),
                message: "Input must be for this product.".into(),
                code: Some("MUST_BE_FOR_THIS_PRODUCT".into()),
            });
        }
        if input.contains_key("inventoryQuantities") {
            own_errors.push(UserError {
                field: Some(
                    path.iter()
                        .cloned()
                        .chain(["inventoryQuantities".to_string()])
                        .collect(),
                ),
                message: "Inventory quantities can only be provided during create. To update \
                          inventory for existing variants, use inventoryAdjustQuantities."
                    .into(),
                code: Some("NO_INVENTORY_QUANTITIES_ON_VARIANTS_UPDATE".into()),
            });
        }
        check_fields(input, &path, &mut own_errors);
        let mut adjustments = Vec::new();
        for (position, adjustment) in objects(input, "quantityAdjustments").iter().enumerate() {
            let location = text(adjustment, "locationId").unwrap_or_default();
            let at: Vec<String> = path
                .iter()
                .cloned()
                .chain([
                    "quantityAdjustments".to_string(),
                    position.to_string(),
                    "locationId".to_string(),
                ])
                .collect();
            let level =
                item_of(state, &id).and_then(|item| records::level_at(state, &item, location));
            match level {
                Some(level) => {
                    let current = state
                        .get(&level)
                        .map_or(0, |record| records::quantity(record, "available"));
                    if let Some(expected) = adjustment.get("changeFromQuantity")
                        && let Some(expected) = expected.as_i64()
                        && expected != current
                    {
                        own_errors.push(UserError {
                            field: Some(at.clone()),
                            message: "The changeFromQuantity argument no longer matches the persisted quantity.".into(),
                            code: Some("INVALID_INPUT".into()),
                        });
                    }
                    let delta = adjustment
                        .get("adjustment")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    adjustments.push((level, delta));
                }
                None => own_errors.push(UserError {
                    field: Some(at),
                    message: "Quantity could not be set. The location was not found.".into(),
                    code: Some("INVALID_INPUT".into()),
                }),
            }
        }
        let values = objects(input, "optionValues");
        let mut chosen = None;
        if !values.is_empty() {
            match choices(&options, &values, &path, Reader::BulkUpdate) {
                Ok(choices) => {
                    let key = combination(&choices);
                    if combinations
                        .iter()
                        .any(|(other, combination)| *other != id && *combination == key)
                    {
                        own_errors.push(UserError {
                            field: Some(path.clone()),
                            message: format!(
                                "The variant '{}' already exists.",
                                title_of(&choices)
                            ),
                            code: Some("VARIANT_ALREADY_EXISTS".into()),
                        });
                    } else if let Some(entry) =
                        combinations.iter_mut().find(|(other, _)| *other == id)
                    {
                        entry.1 = key;
                    }
                    chosen = Some(choices);
                }
                Err(found) => own_errors.extend(found),
            }
        }
        if own_errors.is_empty() {
            planned.push((id, *input, chosen, adjustments));
        }
        errors.extend(own_errors);
    }
    if media_ok.contains(&false) || (!errors.is_empty() && (!partial || planned.is_empty())) {
        return Ok(payload(empty, &errors));
    }

    let state = mutation.state_mut();
    for input in media::media_arg(ctx) {
        media::create_media(state, &product, input);
    }
    let before = records::stock_of(state, std::slice::from_ref(&product));
    let mut updated = Vec::new();
    let mut events: Vec<(&str, String)> = Vec::new();
    for (variant, input, chosen, adjustments) in &planned {
        if let Some(choices) = chosen {
            let choices = ensure_values(state, choices);
            set_choices(state, variant, &choices);
        }
        if apply_fields(state, variant, input)
            && let Some(item) = item_of(state, variant)
        {
            state.touch(&item);
            events.push(("inventory_items/update", item));
        }
        for (level, delta) in adjustments {
            records::change_quantity(state, level, "available", *delta);
            events.push(("inventory_levels/update", level.clone()));
        }
        let attached = media::media_of_variant_input(state, &product, input);
        if !attached.is_empty() {
            media::attach_to_variant(state, variant, &attached);
        }
        let metafields = objects(input, "metafields");
        records::set_metafields(state, variant, "PRODUCTVARIANT", &metafields);
        state.touch(variant);
        updated.push(variant.clone());
    }
    product_changed(state, &product);
    records::after_inventory_change(state, std::slice::from_ref(&product), &before);
    state.touch(&product);
    for (topic, gid) in &events {
        state.emit(topic, gid);
    }
    state.emit("products/update", &product);
    if !errors.is_empty() {
        // `allowPartialUpdates`: the valid variants are updated, the others reported.
        mutation.keep_changes();
    }
    Ok(payload(
        json!({"product": product, "productVariants": updated}),
        &errors,
    ))
}

/// `productVariantsBulkDelete`.
fn bulk_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(product) = product_arg(state, ctx) else {
        return Ok(payload(json!({"product": null}), &[no_product()]));
    };
    let own = state
        .get(&product)
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default();
    let ids = gids(ctx.arg("variantsIds"));
    let mut errors = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        if !own.contains(id) {
            // shopify.dev's example: `field` `["variantsIds", "1"]`.
            errors.push(coded(
                &["variantsIds", &index.to_string()],
                "At least one variant does not belong to the product",
                "AT_LEAST_ONE_VARIANT_DOES_NOT_BELONG_TO_THE_PRODUCT",
            ));
        }
    }
    if errors.is_empty() && own.iter().all(|variant| ids.contains(variant)) {
        errors.push(coded(
            &["variantsIds"],
            "Cannot delete default variant.",
            "CANNOT_DELETE_LAST_VARIANT",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"product": null}), &errors));
    }
    let state = mutation.state_mut();
    for variant in &ids {
        remove_variant(state, &product, variant);
    }
    product_changed(state, &product);
    state.touch(&product);
    state.emit("products/update", &product);
    Ok(payload(json!({"product": product}), &[]))
}

/// `productVariantsBulkReorder`: each variant named takes its `position` (from 1), the others
/// keep their order around them.
fn bulk_reorder(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(product) = product_arg(state, ctx) else {
        return Ok(payload(json!({"product": null}), &[no_product()]));
    };
    let mut variants = state
        .get(&product)
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default();
    let positions = arg_objects(ctx, "positions");
    let mut errors = Vec::new();
    let mut moves: Vec<(String, usize)> = Vec::new();
    for (index, position) in positions.iter().enumerate() {
        let at = index.to_string();
        let id = text(position, "id").unwrap_or_default().to_string();
        let place = position
            .get("position")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if !variants.contains(&id) {
            errors.push(coded(
                &["positions", &at, "id"],
                "Product variant does not exist.",
                "MISSING_VARIANT",
            ));
        }
        if place < 1 {
            errors.push(coded(
                &["positions", &at, "position"],
                "Product variant position cannot be zero or negative number.",
                "INVALID_POSITION",
            ));
        }
        if moves.iter().any(|(known, _)| *known == id) {
            errors.push(coded(
                &["positions", &at, "id"],
                "Product variant IDs must be unique.",
                "DUPLICATED_VARIANT_ID",
            ));
        }
        moves.push((id, usize::try_from(place.max(1) - 1).unwrap_or(0)));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"product": null}), &errors));
    }
    moves.sort_by_key(|(_, place)| *place);
    variants.retain(|variant| !moves.iter().any(|(moved, _)| moved == variant));
    for (variant, place) in moves {
        let place = place.min(variants.len());
        variants.insert(place, variant);
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&product) {
        record.insert("variants".into(), json!(variants));
    }
    product_changed(state, &product);
    state.touch(&product);
    state.emit("products/update", &product);
    Ok(payload(json!({"product": product}), &[]))
}
