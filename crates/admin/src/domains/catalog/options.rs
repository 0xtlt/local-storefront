//! The options of a product: `productOptionsCreate`, `productOptionUpdate`,
//! `productOptionsDelete`, `productOptionsReorder`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A product has at most 3 options ("Can only specify a maximum of 3 options", code
//!   `OPTIONS_OVER_LIMIT`, field `["options"]`), with distinct names ("Option 'Color' already
//!   exists.", `OPTION_ALREADY_EXISTS`, field `["options", "0"]`): the reference's examples.
//! - `productOptionsCreate` puts the new options at their `position` (else last); the existing
//!   variants take the first value of each (`LEAVE_AS_IS`, the default), and `CREATE` adds a
//!   variant for each other value with each existing variant (the strategy's description). On a
//!   product with only the default variant, the option "Title" gives way to the new ones.
//! - `productOptionUpdate` renames and moves an option, adds values (with `MANAGE`, a variant
//!   for each with each combination of the other options), renames values (the variants that
//!   select them follow), deletes values (refused while variants select them, unless `MANAGE`,
//!   which deletes those variants). "Option value already exists." (`OPTION_VALUE_ALREADY_EXISTS`,
//!   field `["optionValuesToAdd", "1", "name"]`) is the reference's example.
//! - `productOptionsDelete` removes options: by default only one with a single value ("Cannot
//!   delete an option with multiple values.", `CANNOT_DELETE_OPTION_WITH_MULTIPLE_VALUES`, field
//!   `["options"]`, the reference's example); `NON_DESTRUCTIVE` when no variant would become a
//!   duplicate of another; `POSITION` deletes the duplicates, the last ones first. Deleting every
//!   option leaves the product with its default variant.
//! - `productOptionsReorder` reorders the options and their values; a value left out is refused
//!   ("Missing option value '155cm'.", `MISSING_OPTION_VALUE`, the reference's example), and the
//!   variants follow the new order of the values (the example's response).
//! - Events: `products/update`, and the inventory events of the variants created or deleted.

use serde_json::{Map, Value, json};

use super::records::{arg_objects, coded, field, is, objects, text};
use super::variants::{
    self, Choice, ProductOption, add_value, no_product, options_of, product_arg, remove_variant,
};
use super::{gids, product_changed};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, payload};
use crate::state::ShopState;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("productOptionsCreate", options_create)
        .mutation("productOptionUpdate", option_update)
        .mutation("productOptionsDelete", options_delete)
        .mutation("productOptionsReorder", options_reorder);
}

/// The most options a product has.
const OPTION_LIMIT: usize = 3;

/// The variants of a product, with what they select: `(variant, [(option GID, value GID)])`.
fn selections(state: &ShopState, product: &str) -> Vec<(String, Vec<(String, String)>)> {
    let options = options_of(state, product);
    state
        .get(product)
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default()
        .into_iter()
        .map(|variant| {
            let selected: Vec<(String, String)> = state
                .get(&variant)
                .and_then(|record| record.get("selectedOptions"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|selected| {
                    let name = selected.get("name")?.as_str()?;
                    let option = options.iter().find(|option| option.name == name)?;
                    let value = selected
                        .get("optionValue")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| {
                            let value = selected.get("value")?.as_str()?;
                            option
                                .values
                                .iter()
                                .find(|(_, known)| known == value)
                                .map(|(id, _)| id.clone())
                        })?;
                    Some((option.id.clone(), value))
                })
                .collect();
            (variant, selected)
        })
        .collect()
}

/// The choices of a selection `[(option GID, value GID)]`, in the order of the options, with
/// the values' current names.
fn choices_of(options: &[ProductOption], selected: &[(String, String)]) -> Vec<Choice> {
    options
        .iter()
        .filter_map(|option| {
            let (_, value) = selected.iter().find(|(known, _)| *known == option.id)?;
            let name = option.values.iter().find(|(id, _)| id == value)?.1.clone();
            Some(Choice {
                option: option.id.clone(),
                name,
                value: Some(value.clone()),
            })
        })
        .collect()
}

/// Writes what each variant selects, in the order of the product's options (and the value's
/// current name).
fn write_selections(
    state: &mut ShopState,
    product: &str,
    selections: &[(String, Vec<(String, String)>)],
) {
    let options = options_of(state, product);
    for (variant, selected) in selections {
        variants::set_choices(state, variant, &choices_of(&options, selected));
    }
}

/// Keeps an option's `values` (names) in the order of its `optionValues`.
fn sync_values(state: &mut ShopState, option: &str) {
    let names: Vec<String> = state
        .get(option)
        .map(|record| gids(record.get("optionValues")))
        .unwrap_or_default()
        .iter()
        .filter_map(|value| Some(field(state.get(value)?, "name")?.to_string()))
        .collect();
    if let Some(record) = state.get_mut(option) {
        record.insert("values".into(), json!(names));
    }
}

/// Writes the options of a product in this order, with their positions.
fn set_options(state: &mut ShopState, product: &str, options: &[String]) {
    for (index, option) in options.iter().enumerate() {
        if let Some(record) = state.get_mut(option) {
            record.insert("position".into(), json!(index + 1));
        }
    }
    if let Some(record) = state.get_mut(product) {
        record.insert("options".into(), json!(options));
    }
}

/// A new option with its values. Returns its GID and the GIDs of its values.
fn new_option(state: &mut ShopState, name: &str, values: &[String]) -> (String, Vec<String>) {
    let now = state.timestamp();
    let option = variants::add_option(state, name, 1, &now);
    let ids = values
        .iter()
        .map(|value| add_value(state, &option, value))
        .collect();
    (option, ids)
}

/// Creates a variant like `like` (its price and inventory settings), selecting `selected`.
fn variant_like(
    state: &mut ShopState,
    product: &str,
    like: &str,
    selected: &[(String, String)],
) -> String {
    let record = state.get(like).cloned().unwrap_or_default();
    let mut input = Map::new();
    for name in ["price", "compareAtPrice", "inventoryPolicy", "taxable"] {
        if let Some(value) = record.get(name).filter(|value| !value.is_null()) {
            input.insert(name.into(), value.clone());
        }
    }
    let choices = choices_of(&options_of(state, product), selected);
    let stock = variants::default_stock(state);
    variants::insert_variant(state, product, &choices, &input, &stock)
}

/// The names of a list of value inputs, trimmed.
fn value_names(values: &[&Map<String, Value>]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| text(value, "name"))
        .map(|name| name.trim().to_string())
        .collect()
}

/// `productOptionsCreate`.
fn options_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(product) = product_arg(state, ctx) else {
        return Ok(payload(json!({"product": null}), &[no_product()]));
    };
    let existing = options_of(state, &product);
    let default_only = state
        .get(&product)
        .and_then(|record| record.get("hasOnlyDefaultVariant"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let kept = if default_only { 0 } else { existing.len() };
    let inputs = arg_objects(ctx, "options");
    let mut errors = Vec::new();
    if kept + inputs.len() > OPTION_LIMIT {
        errors.push(coded(
            &["options"],
            "Can only specify a maximum of 3 options",
            "OPTIONS_OVER_LIMIT",
        ));
    }
    let mut names: Vec<String> = if default_only {
        Vec::new()
    } else {
        existing
            .iter()
            .map(|option| option.name.to_lowercase())
            .collect()
    };
    let mut planned = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let at = index.to_string();
        let name = text(input, "name").unwrap_or_default().trim().to_string();
        if name.is_empty() {
            errors.push(coded(
                &["options", &at, "name"],
                "Each option must have a name specified.",
                "OPTION_NAME_MISSING",
            ));
        } else if names.contains(&name.to_lowercase()) {
            errors.push(coded(
                &["options", &at],
                &format!("Option '{name}' already exists."),
                "OPTION_ALREADY_EXISTS",
            ));
        }
        names.push(name.to_lowercase());
        let values = value_names(&objects(input, "values"));
        if values.is_empty() {
            errors.push(coded(
                &["options", &at, "values"],
                "Each option must have at least one option value specified.",
                "OPTION_VALUES_MISSING",
            ));
        }
        let mut seen: Vec<String> = Vec::new();
        for value in &values {
            if seen.contains(&value.to_lowercase()) {
                errors.push(coded(
                    &["options", &at, "values"],
                    "Duplicated option value.",
                    "DUPLICATED_OPTION_VALUE",
                ));
            }
            seen.push(value.to_lowercase());
        }
        let position = input.get("position").and_then(Value::as_i64);
        if let Some(position) = position
            && !(1..=OPTION_LIMIT as i64).contains(&position)
        {
            errors.push(coded(
                &["options", &at, "position"],
                "Position must be between 1 and the maximum number of options per product.",
                "POSITION_OUT_OF_BOUNDS",
            ));
        }
        planned.push((name, values, position));
    }
    let strategy = ctx.arg_str("variantStrategy").unwrap_or("LEAVE_AS_IS");
    let variant_count = state
        .get(&product)
        .map_or(0, |record| gids(record.get("variants")).len());
    if strategy == "CREATE" {
        let multiplier: usize = planned
            .iter()
            .map(|(_, values, _)| values.len().max(1))
            .product();
        if variant_count * multiplier > variants::VARIANT_LIMIT {
            errors.push(coded(
                &["options"],
                "The number of option values created with the CREATE strategy would exceed the variant limit.",
                "TOO_MANY_VARIANTS_CREATED",
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"product": null}), &errors));
    }
    if inputs
        .iter()
        .any(|input| input.contains_key("linkedMetafield"))
    {
        mutation.note(
            "productOptionsCreate: lsf does not simulate `linkedMetafield` yet: it was ignored",
        );
    }

    let state = mutation.state_mut();
    let mut selections = selections(state, &product);
    let mut order: Vec<String> = if default_only {
        // The default "Title" option gives way.
        for option in &existing {
            for (value, _) in &option.values {
                state.remove(value);
            }
            state.remove(&option.id);
        }
        for (_, selected) in &mut selections {
            selected.clear();
        }
        Vec::new()
    } else {
        existing.iter().map(|option| option.id.clone()).collect()
    };
    let mut created_options = Vec::new();
    for (name, values, position) in &planned {
        let (option, value_ids) = new_option(state, name, values);
        let at = position
            .map(|position| usize::try_from(position - 1).unwrap_or(0))
            .unwrap_or(order.len())
            .min(order.len());
        order.insert(at, option.clone());
        for (_, selected) in &mut selections {
            if let Some(first) = value_ids.first() {
                selected.push((option.clone(), first.clone()));
            }
        }
        created_options.push((option, value_ids));
    }
    set_options(state, &product, &order);
    write_selections(state, &product, &selections);
    let mut created = Vec::new();
    if strategy == "CREATE" {
        // Each existing variant, with every other value of the new options.
        let mut combos: Vec<(String, Vec<(String, String)>)> = selections.clone();
        for (option, value_ids) in &created_options {
            let mut next = Vec::new();
            for (like, selected) in &combos {
                for value in value_ids.iter().skip(1) {
                    let mut other = selected.clone();
                    if let Some(entry) = other.iter_mut().find(|(known, _)| known == option) {
                        entry.1 = value.clone();
                    }
                    next.push((like.clone(), other));
                }
            }
            combos.extend(next);
        }
        for (like, selected) in combos.into_iter().skip(selections.len()) {
            created.push(variant_like(state, &product, &like, &selected));
        }
    }
    product_changed(state, &product);
    state.touch(&product);
    for variant in &created {
        super::products::emit_new_inventory(state, variant);
    }
    state.emit("products/update", &product);
    Ok(payload(json!({"product": product}), &[]))
}

/// `productOptionUpdate`.
fn option_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(product) = product_arg(state, ctx) else {
        return Ok(payload(json!({"product": null}), &[no_product()]));
    };
    let options = options_of(state, &product);
    let option_input = super::records::input(ctx, "option");
    let option_id = text(&option_input, "id").unwrap_or_default().to_string();
    let Some(option) = options
        .iter()
        .find(|option| option.id == option_id)
        .cloned()
    else {
        let error = coded(
            &["option", "id"],
            "Option does not exist.",
            "OPTION_DOES_NOT_EXIST",
        );
        return Ok(payload(json!({"product": null}), &[error]));
    };
    let strategy = ctx.arg_str("variantStrategy").unwrap_or("LEAVE_AS_IS");
    let to_add = value_names(&arg_objects(ctx, "optionValuesToAdd"));
    let to_update = arg_objects(ctx, "optionValuesToUpdate");
    let to_delete = gids(ctx.arg("optionValuesToDelete"));
    let selections = selections(state, &product);
    let mut errors = Vec::new();
    if let Some(name) = text(&option_input, "name").map(str::trim) {
        if name.is_empty() {
            errors.push(coded(
                &["option", "name"],
                "The name provided is not valid.",
                "INVALID_NAME",
            ));
        } else if options
            .iter()
            .any(|other| other.id != option.id && other.name.eq_ignore_ascii_case(name))
        {
            errors.push(coded(
                &["option", "name"],
                &format!("Option '{name}' already exists."),
                "OPTION_ALREADY_EXISTS",
            ));
        }
    }
    if let Some(position) = option_input.get("position").and_then(Value::as_i64)
        && !(1..=options.len() as i64).contains(&position)
    {
        errors.push(coded(
            &["option", "position"],
            "The option position provided is not valid.",
            "INVALID_POSITION",
        ));
    }
    let mut names: Vec<String> = option
        .values
        .iter()
        .filter(|(id, _)| !to_delete.contains(id))
        .map(|(_, name)| name.to_lowercase())
        .collect();
    for (index, name) in to_add.iter().enumerate() {
        if name.is_empty() || names.contains(&name.to_lowercase()) {
            // shopify.dev's example.
            errors.push(coded(
                &["optionValuesToAdd", &index.to_string(), "name"],
                "Option value already exists.",
                "OPTION_VALUE_ALREADY_EXISTS",
            ));
        }
        names.push(name.to_lowercase());
    }
    for (index, update) in to_update.iter().enumerate() {
        let id = text(update, "id").unwrap_or_default();
        if !option.values.iter().any(|(known, _)| known == id) {
            errors.push(coded(
                &["optionValuesToUpdate", &index.to_string(), "id"],
                "Option value does not exist.",
                "OPTION_VALUE_DOES_NOT_EXIST",
            ));
        }
        if to_delete.iter().any(|deleted| deleted == id) {
            errors.push(coded(
                &["optionValuesToUpdate", &index.to_string(), "id"],
                "Performing conflicting actions on an option value.",
                "OPTION_VALUE_CONFLICTING_OPERATION",
            ));
        }
        if let Some(name) = text(update, "name").map(str::trim) {
            let current = option
                .values
                .iter()
                .find(|(known, _)| known == id)
                .map(|(_, name)| name.to_lowercase());
            if current.as_deref() != Some(&name.to_lowercase())
                && names.contains(&name.to_lowercase())
            {
                errors.push(coded(
                    &["optionValuesToUpdate", &index.to_string(), "name"],
                    "Option value already exists.",
                    "OPTION_VALUE_ALREADY_EXISTS",
                ));
            }
        }
    }
    for (index, id) in to_delete.iter().enumerate() {
        if !option.values.iter().any(|(known, _)| known == id) {
            errors.push(coded(
                &["optionValuesToDelete", &index.to_string()],
                "Option value does not exist.",
                "OPTION_VALUE_DOES_NOT_EXIST",
            ));
        } else if strategy != "MANAGE"
            && selections
                .iter()
                .any(|(_, selected)| selected.iter().any(|(_, value)| value == id))
        {
            errors.push(coded(
                &["optionValuesToDelete", &index.to_string()],
                "Option value with variants linked cannot be deleted.",
                "OPTION_VALUE_HAS_VARIANTS",
            ));
        }
    }
    if !to_delete.is_empty()
        && option.values.iter().all(|(id, _)| to_delete.contains(id))
        && to_add.is_empty()
    {
        errors.push(coded(
            &["optionValuesToDelete"],
            "Deleting all option values of an option is not allowed.",
            "CANNOT_DELETE_ALL_OPTION_VALUES_IN_OPTION",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"product": null}), &errors));
    }

    let state = mutation.state_mut();
    if let Some(name) = text(&option_input, "name").map(str::trim)
        && let Some(record) = state.get_mut(&option.id)
    {
        record.insert("name".into(), json!(name));
    }
    for update in &to_update {
        let id = text(update, "id").unwrap_or_default().to_string();
        if let Some(name) = text(update, "name").map(str::trim)
            && let Some(record) = state.get_mut(&id)
        {
            record.insert("name".into(), json!(name));
        }
    }
    let added: Vec<String> = to_add
        .iter()
        .map(|name| add_value(state, &option.id, name))
        .collect();
    // The variants of the values deleted go with them (`MANAGE`).
    let mut removed = Vec::new();
    for (variant, selected) in &selections {
        if selected.iter().any(|(_, value)| to_delete.contains(value)) {
            remove_variant(state, &product, variant);
            removed.push(variant.clone());
        }
    }
    for id in &to_delete {
        if let Some(Value::Array(values)) = state
            .get_mut(&option.id)
            .and_then(|record| record.get_mut("optionValues"))
        {
            values.retain(|known| known.as_str() != Some(id.as_str()));
        }
        state.remove(id);
    }
    sync_values(state, &option.id);
    let mut order: Vec<String> = options.iter().map(|known| known.id.clone()).collect();
    if let Some(position) = option_input.get("position").and_then(Value::as_i64) {
        order.retain(|known| *known != option.id);
        let at = usize::try_from(position - 1).unwrap_or(0).min(order.len());
        order.insert(at, option.id.clone());
    }
    set_options(state, &product, &order);
    let remaining: Vec<(String, Vec<(String, String)>)> = selections
        .iter()
        .filter(|(variant, _)| !removed.contains(variant))
        .cloned()
        .collect();
    write_selections(state, &product, &remaining);
    let mut created = Vec::new();
    if strategy == "MANAGE" && !added.is_empty() {
        // A variant for each new value with each combination of the other options.
        let mut seen: Vec<Vec<(String, String)>> = Vec::new();
        for (like, selected) in &remaining {
            let others: Vec<(String, String)> = selected
                .iter()
                .filter(|(known, _)| *known != option.id)
                .cloned()
                .collect();
            if seen.contains(&others) {
                continue;
            }
            seen.push(others.clone());
            for value in &added {
                let mut combination = others.clone();
                combination.push((option.id.clone(), value.clone()));
                created.push(variant_like(state, &product, like, &combination));
            }
        }
    }
    product_changed(state, &product);
    state.touch(&product);
    for variant in &created {
        super::products::emit_new_inventory(state, variant);
    }
    state.emit("products/update", &product);
    Ok(payload(json!({"product": product}), &[]))
}

/// `productOptionsDelete`.
fn options_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"product": null, "deletedOptionsIds": null});
    let state = mutation.state();
    let Some(product) = product_arg(state, ctx) else {
        return Ok(payload(empty, &[no_product()]));
    };
    let options = options_of(state, &product);
    let ids = gids(ctx.arg("options"));
    let strategy = ctx.arg_str("strategy").unwrap_or("DEFAULT");
    let mut errors = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        match options.iter().find(|option| option.id == *id) {
            None if is(state, id, "ProductOption") => errors.push(coded(
                &["options", &index.to_string()],
                "Options do not belong to the same product.",
                "OPTIONS_DO_NOT_BELONG_TO_THE_SAME_PRODUCT",
            )),
            None => errors.push(coded(
                &["options", &index.to_string()],
                "Option does not exist.",
                "OPTION_DOES_NOT_EXIST",
            )),
            Some(option) if strategy == "DEFAULT" && option.values.len() > 1 => {
                // shopify.dev's example.
                errors.push(coded(
                    &["options"],
                    "Cannot delete an option with multiple values.",
                    "CANNOT_DELETE_OPTION_WITH_MULTIPLE_VALUES",
                ));
            }
            Some(_) => {}
        }
    }
    let selections = selections(state, &product);
    // What each variant selects without the deleted options; the duplicates go, the last first.
    let mut kept: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut duplicates = Vec::new();
    for (variant, selected) in &selections {
        let rest: Vec<(String, String)> = selected
            .iter()
            .filter(|(option, _)| !ids.contains(option))
            .cloned()
            .collect();
        if kept.iter().any(|(_, other)| *other == rest) {
            duplicates.push(variant.clone());
        } else {
            kept.push((variant.clone(), rest));
        }
    }
    if errors.is_empty() && strategy == "NON_DESTRUCTIVE" && !duplicates.is_empty() {
        errors.push(coded(
            &["options"],
            "Cannot delete options without deleting variants.",
            "CANNOT_USE_NON_DESTRUCTIVE_STRATEGY",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }

    let state = mutation.state_mut();
    for variant in &duplicates {
        remove_variant(state, &product, variant);
    }
    for option in options.iter().filter(|option| ids.contains(&option.id)) {
        for (value, _) in &option.values {
            state.remove(value);
        }
        state.remove(&option.id);
    }
    let mut order: Vec<String> = options
        .iter()
        .map(|option| option.id.clone())
        .filter(|option| !ids.contains(option))
        .collect();
    if order.is_empty() {
        // No option left: the product is back to its default variant.
        let (option, values) = new_option(state, "Title", &["Default Title".to_string()]);
        order.push(option.clone());
        if let Some(first) = values.first() {
            for (_, selected) in &mut kept {
                selected.push((option.clone(), first.clone()));
            }
        }
    }
    set_options(state, &product, &order);
    write_selections(state, &product, &kept);
    product_changed(state, &product);
    state.touch(&product);
    state.emit("products/update", &product);
    Ok(payload(
        json!({"product": product, "deletedOptionsIds": ids}),
        &[],
    ))
}

/// The option an `OptionReorderInput` names.
fn named_option<'o>(
    options: &'o [ProductOption],
    input: &Map<String, Value>,
) -> Option<&'o ProductOption> {
    match (text(input, "id"), text(input, "name")) {
        (Some(id), _) => options.iter().find(|option| option.id == id),
        (None, Some(name)) => options.iter().find(|option| option.name == name),
        _ => None,
    }
}

/// `productOptionsReorder`.
fn options_reorder(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(product) = product_arg(state, ctx) else {
        return Ok(payload(json!({"product": null}), &[no_product()]));
    };
    let options = options_of(state, &product);
    let inputs = arg_objects(ctx, "options");
    let mut errors = Vec::new();
    let mut order: Vec<(String, Vec<String>)> = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let at = index.to_string();
        let Some(option) = named_option(&options, input) else {
            let (message, code) = if input.contains_key("id") {
                ("Option id does not exist.", "OPTION_ID_DOES_NOT_EXIST")
            } else {
                ("Option name does not exist.", "OPTION_NAME_DOES_NOT_EXIST")
            };
            errors.push(coded(&["options", &at], message, code));
            continue;
        };
        if order.iter().any(|(known, _)| *known == option.id) {
            errors.push(coded(
                &["options", &at],
                "Duplicated option name.",
                "DUPLICATED_OPTION_NAME",
            ));
            continue;
        }
        let values = objects(input, "values");
        let mut value_order = Vec::new();
        if values.is_empty() {
            value_order = option.values.iter().map(|(id, _)| id.clone()).collect();
        } else {
            for (position, value) in values.iter().enumerate() {
                let found = match (text(value, "id"), text(value, "name")) {
                    (Some(id), _) => option.values.iter().find(|(known, _)| known == id),
                    (None, Some(name)) => option.values.iter().find(|(_, known)| known == name),
                    _ => None,
                };
                match found {
                    Some((id, _)) if !value_order.contains(id) => value_order.push(id.clone()),
                    Some(_) => errors.push(coded(
                        &["options", &at, "values", &position.to_string()],
                        "Duplicated option value.",
                        "DUPLICATED_OPTION_VALUE",
                    )),
                    None => errors.push(coded(
                        &["options", &at, "values", &position.to_string()],
                        "Option value does not exist.",
                        "OPTION_VALUE_DOES_NOT_EXIST",
                    )),
                }
            }
            for (id, name) in &option.values {
                if !value_order.contains(id) {
                    // shopify.dev's example.
                    errors.push(coded(
                        &["options"],
                        &format!("Missing option value '{name}'."),
                        "MISSING_OPTION_VALUE",
                    ));
                }
            }
        }
        order.push((option.id.clone(), value_order));
    }
    for option in &options {
        if !order.iter().any(|(known, _)| *known == option.id) {
            errors.push(coded(
                &["options"],
                "Missing option name.",
                "MISSING_OPTION_NAME",
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"product": null}), &errors));
    }

    let mut selections = selections(state, &product);
    let state = mutation.state_mut();
    for (option, values) in &order {
        if let Some(record) = state.get_mut(option) {
            record.insert("optionValues".into(), json!(values));
        }
        sync_values(state, option);
    }
    let option_ids: Vec<String> = order.iter().map(|(id, _)| id.clone()).collect();
    set_options(state, &product, &option_ids);
    write_selections(state, &product, &selections);
    // The variants follow the order of the options' values.
    let rank = |selected: &Vec<(String, String)>| -> Vec<usize> {
        order
            .iter()
            .map(|(option, values)| {
                selected
                    .iter()
                    .find(|(known, _)| known == option)
                    .and_then(|(_, value)| values.iter().position(|known| known == value))
                    .unwrap_or(usize::MAX)
            })
            .collect()
    };
    selections.sort_by_key(|(_, selected)| rank(selected));
    let variants: Vec<&String> = selections.iter().map(|(variant, _)| variant).collect();
    if let Some(record) = state.get_mut(&product) {
        record.insert("variants".into(), json!(variants));
    }
    product_changed(state, &product);
    state.touch(&product);
    state.emit("products/update", &product);
    Ok(payload(json!({"product": product}), &[]))
}
