//! Combined listings: `combinedListingUpdate`, `Product.combinedListing`, and the
//! `combinedListingRole` a product is created with (`productCreate`, `productSet`).
//!
//! As shopify.dev describes them ("Build for combined listings", read on 2026-10-31, and the
//! reference of 2026-10):
//!
//! - "A combined listing is modeled on a single parent product that has variants that point to
//!   one or more child products": the parent is created with `combinedListingRole: PARENT`
//!   ("You can specify this field only when you create a product"), then `combinedListingUpdate`
//!   adds children (`productsAdded`, each with the parent's option values it stands for),
//!   changes their values (`productsEdited`), removes them (`productsRemovedIds`) and orders the
//!   parent's options and values (`optionsAndValues`; "By default, new colors are added to the
//!   end of the parent product's option values"). A child gets `combinedListingRole: CHILD`.
//! - `Product.combinedListing` answers the parent and its children (`combinedListingChildren`:
//!   each child's `product` and the `parentVariant` that stands for it), for the parent and for
//!   each child; `null` for a product in none. `products(query: "combined_listing_role:PARENT")`
//!   reads the role.
//! - The errors are the codes of `CombinedListingUpdateUserErrorCode`, their descriptions the
//!   messages ("Unable to add a product that is already a child.").
//!
//! A parent variant costs what its child's cheapest variant does and tracks no inventory (lsf's
//! choice: shopify.dev does not say how a parent variant is priced). The option values of a
//! linked metafield (`linkedMetafieldValue`) are kept on the parent's values. Events:
//! `products/update` for the parent and for each child whose role changed. Combined listings do
//! not reach the storefront: the store data has no such relation, and the parent syncs as a
//! product of its own.

use serde_json::{Map, Value, json};

use super::records::{field, text};
use super::variants::{self, Choice};
use super::{cents_of, gids, product_changed};
use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::admin::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("combinedListingUpdate", combined_listing_update)
        .field("Product", "combinedListing", combined_listing)
        // `null` for a product in no combined listing: what the seed's products are.
        .field("Product", "combinedListingRole", |r, _ctx, object| {
            let role = r
                .map_of(object)
                .and_then(|record| record.get("combinedListingRole"))
                .cloned()
                .unwrap_or(Value::Null);
            Ok(Output::Value(role))
        });
}

/// The record field of a parent that lists its children: `[{"product", "parentVariant"}]`.
const CHILDREN: &str = "lsfCombinedListing";

/// The record field of a child that names its parent.
const PARENT: &str = "lsfCombinedListingParent";

/// The most children a combined listing has (Shopify's limit is not published: lsf's).
const MAX_CHILDREN: usize = 2048;

/// The longest title.
const TITLE_LENGTH: usize = 255;

/// An error of `combinedListingUpdate`, by its code: the code's description is its message.
fn error(field: &[&str], code: &str) -> UserError {
    let message = match code {
        "CANNOT_HAVE_DUPLICATED_PRODUCTS" => "Unable to add duplicated products.",
        "CANNOT_HAVE_PARENT_AS_CHILD" => "Unable to add a product that is a parent.",
        "CANNOT_HAVE_REPEATED_OPTIONS" => "Unable to add products with repeated options.",
        "CANNOT_HAVE_REPEATED_OPTION_VALUES" => "Option values cannot be repeated.",
        "CANT_ADD_OPTIONS_VALUES_IF_ALREADY_EXISTS" => {
            "Unable to add options values that are already in use."
        }
        "EDIT_AND_REMOVE_ON_SAME_PRODUCTS" => "Cannot perform edit and remove on same products.",
        "MUST_HAVE_SELECTED_OPTION_VALUES" => "Selected option values cannot be empty.",
        "OPTIONS_MUST_BE_EQUAL_TO_THE_OTHER_COMPONENTS" => {
            "All child products must include the same options."
        }
        "OPTION_NAME_CANNOT_BE_BLANK" => "Unable to add products with blank option names.",
        "OPTION_VALUES_CANNOT_BE_BLANK" => "Unable to update options with blank option values.",
        "OPTION_VALUES_CANNOT_BE_EMPTY" => "Unable to update options with no option values.",
        "OPTION_VALUES_MUST_BE_COMPLETE" => {
            "The options_and_values field must contain all option values used in the combined listing."
        }
        "PARENT_PRODUCT_CANNOT_BE_COMBINED_LISTING_CHILD" => {
            "Parent product cannot be a combined listing child."
        }
        "PARENT_PRODUCT_MUST_BE_A_COMBINED_LISTING" => {
            "Unable to update components for a product that isn't a combined listing."
        }
        "PARENT_PRODUCT_NOT_FOUND" => "Parent product not found.",
        "PRODUCT_IS_ALREADY_A_CHILD" => "Unable to add a product that is already a child.",
        "PRODUCT_MEMBERSHIP_NOT_FOUND" => "Failed to remove mebmership due to invalid input.",
        "PRODUCT_NOT_FOUND" => "Unable to add products that do not exist.",
        "TITLE_TOO_LONG" => "The title cannot be longer than 255 characters.",
        "TOO_MANY_PRODUCTS" => {
            "You have reached the maximum number of products that can be added to an individual combined listing."
        }
        _ => "An unexpected error occurred.",
    };
    UserError::new(field, message).code(code)
}

/// The role of a product in a combined listing.
fn role(state: &ShopState, product: &str) -> Option<String> {
    state
        .get(product)
        .and_then(|record| field(record, "combinedListingRole"))
        .map(str::to_string)
}

/// The children of a parent: `(child, parent variant)`.
fn children(state: &ShopState, parent: &str) -> Vec<(String, String)> {
    state
        .get(parent)
        .and_then(|record| record.get(CHILDREN))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|child| {
            Some((
                child.get("product")?.as_str()?.to_string(),
                child.get("parentVariant")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

/// The selected values of a `ChildProductRelationInput`: `(name, value, linked metafield value)`.
/// The reference's guide gives one object where the schema has a list: both are read.
fn selections(input: &Map<String, Value>) -> Vec<(String, String, Option<String>)> {
    let values = match input.get("selectedParentOptionValues") {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::Object(item)) => vec![Value::Object(item.clone())],
        _ => Vec::new(),
    };
    values
        .iter()
        .filter_map(Value::as_object)
        .map(|value| {
            (
                text(value, "name").unwrap_or_default().trim().to_string(),
                text(value, "value").unwrap_or_default().trim().to_string(),
                text(value, "linkedMetafieldValue").map(str::to_string),
            )
        })
        .collect()
}

/// Checks the selected values of a child at `path`, against the option names the listing uses.
fn check_selections(
    picked: &[(String, String, Option<String>)],
    path: &[&str],
    names: &mut Option<Vec<String>>,
    errors: &mut Vec<UserError>,
) {
    if picked.is_empty() {
        errors.push(error(path, "MUST_HAVE_SELECTED_OPTION_VALUES"));
        return;
    }
    if picked.iter().any(|(name, _, _)| name.is_empty()) {
        errors.push(error(path, "OPTION_NAME_CANNOT_BE_BLANK"));
    }
    if picked.iter().any(|(_, value, _)| value.is_empty()) {
        errors.push(error(path, "OPTION_VALUES_CANNOT_BE_BLANK"));
    }
    let mut own: Vec<String> = picked
        .iter()
        .map(|(name, _, _)| name.to_lowercase())
        .collect();
    let count = own.len();
    own.sort();
    own.dedup();
    if own.len() != count {
        errors.push(error(path, "CANNOT_HAVE_REPEATED_OPTIONS"));
    }
    match names {
        Some(known) if *known != own => {
            errors.push(error(path, "OPTIONS_MUST_BE_EQUAL_TO_THE_OTHER_COMPONENTS"));
        }
        Some(_) => {}
        None => *names = Some(own),
    }
}

/// The option names (lower-case, sorted) a parent's children use.
fn names_in_use(state: &ShopState, parent: &str) -> Option<Vec<String>> {
    let (_, variant) = children(state, parent).into_iter().next()?;
    let mut names: Vec<String> = state
        .get(&variant)?
        .get("selectedOptions")?
        .as_array()?
        .iter()
        .filter_map(|option| option.get("name")?.as_str().map(str::to_lowercase))
        .collect();
    names.sort();
    Some(names)
}

/// The values a parent variant selects, `name → value`, lower-case.
fn combination_of(state: &ShopState, variant: &str) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = state
        .get(variant)
        .and_then(|record| record.get("selectedOptions"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|option| {
            Some((
                option.get("name")?.as_str()?.to_lowercase(),
                option.get("value")?.as_str()?.to_lowercase(),
            ))
        })
        .collect();
    values.sort();
    values
}

fn combination(picked: &[(String, String, Option<String>)]) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = picked
        .iter()
        .map(|(name, value, _)| (name.to_lowercase(), value.to_lowercase()))
        .collect();
    values.sort();
    values
}

/// The option of a parent with a name (ignoring case), made when it has none.
fn option_named(state: &mut ShopState, parent: &str, name: &str) -> String {
    if let Some(option) = variants::options_of(state, parent)
        .into_iter()
        .find(|option| option.name.eq_ignore_ascii_case(name))
    {
        return option.id;
    }
    let now = state.timestamp();
    let position = state
        .get(parent)
        .map_or(0, |record| gids(record.get("options")).len())
        + 1;
    let option = variants::add_option(state, name, position, &now);
    if let Some(Value::Array(options)) = state
        .get_mut(parent)
        .and_then(|record| record.get_mut("options"))
    {
        options.push(json!(option));
    }
    option
}

/// The choices of a parent variant for a child's values, the options and values made as needed.
fn choices_for(
    state: &mut ShopState,
    parent: &str,
    picked: &[(String, String, Option<String>)],
) -> Vec<Choice> {
    // In the order of the parent's options.
    let mut choices = Vec::new();
    for (name, value, linked) in picked {
        let option = option_named(state, parent, name);
        let existing = variants::options_of(state, parent)
            .into_iter()
            .find(|known| known.id == option)
            .and_then(|known| {
                known
                    .values
                    .into_iter()
                    .find(|(_, known)| known == value)
                    .map(|(id, _)| id)
            });
        let id = existing.unwrap_or_else(|| variants::add_value(state, &option, value));
        if let Some(linked) = linked
            && let Some(record) = state.get_mut(&id)
        {
            record.insert("linkedMetafieldValue".into(), json!(linked));
        }
        choices.push(Choice {
            option,
            name: value.clone(),
            value: Some(id),
        });
    }
    let order: Vec<String> = state
        .get(parent)
        .map(|record| gids(record.get("options")))
        .unwrap_or_default();
    choices.sort_by_key(|choice| order.iter().position(|option| *option == choice.option));
    choices
}

/// The cheapest price of a product's variants, in cents.
fn cheapest(state: &ShopState, product: &str) -> i64 {
    gids(state.get(product).and_then(|record| record.get("variants")))
        .iter()
        .filter_map(|variant| state.get(variant))
        .filter_map(|variant| cents_of(variant.get("price")))
        .min()
        .unwrap_or(0)
}

/// Orders a parent's options and their values as `optionsAndValues` gives them, and drops the
/// values and the options no variant uses.
fn tidy(state: &mut ShopState, parent: &str, order: &[&Map<String, Value>]) {
    let variants_of = gids(state.get(parent).and_then(|record| record.get("variants")));
    let used: Vec<String> = variants_of
        .iter()
        .filter_map(|variant| state.get(variant))
        .flat_map(|variant| {
            variant
                .get("selectedOptions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|option| option.get("optionValue")?.as_str().map(str::to_string))
        .collect();
    // The place of an option in `optionsAndValues`, by name.
    let place = |name: &str| {
        order.iter().position(|input| {
            text(input, "name").is_some_and(|known| known.eq_ignore_ascii_case(name))
        })
    };
    let mut options = variants::options_of(state, parent);
    if !order.is_empty() {
        options.sort_by_key(|option| place(&option.name).unwrap_or(usize::MAX));
    }
    let mut kept = Vec::new();
    for option in &options {
        let input = place(&option.name).map(|index| order[index]);
        let wanted: Vec<String> = input
            .and_then(|input| input.get("values"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        let mut values: Vec<(String, String)> = option
            .values
            .iter()
            .filter(|(id, _)| used.contains(id))
            .cloned()
            .collect();
        if !wanted.is_empty() {
            values.sort_by_key(|(_, name)| {
                wanted.iter().position(|w| w == name).unwrap_or(usize::MAX)
            });
        }
        for (id, _) in &option.values {
            if !used.contains(id) {
                state.remove(id);
            }
        }
        if values.is_empty() {
            state.remove(&option.id);
            continue;
        }
        let linked = input
            .and_then(|input| input.get("linkedMetafield"))
            .filter(|linked| !linked.is_null())
            .map(|linked| json!({"namespace": linked.get("namespace"), "key": linked.get("key")}));
        if let Some(record) = state.get_mut(&option.id) {
            record.insert("position".into(), json!(kept.len() + 1));
            record.insert(
                "optionValues".into(),
                json!(values.iter().map(|(id, _)| id).collect::<Vec<_>>()),
            );
            record.insert(
                "values".into(),
                json!(values.iter().map(|(_, name)| name).collect::<Vec<_>>()),
            );
            if let Some(linked) = linked {
                record.insert("linkedMetafield".into(), linked);
            }
        }
        kept.push(option.id.clone());
    }
    if let Some(record) = state.get_mut(parent) {
        record.insert("options".into(), json!(kept));
    }
    // The selected options of each variant in the options' new order.
    for variant in variants_of {
        let Some(record) = state.get(&variant) else {
            continue;
        };
        let mut selected: Vec<Value> = record
            .get("selectedOptions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        selected.sort_by_key(|option| {
            let value = option
                .get("optionValue")
                .and_then(Value::as_str)
                .unwrap_or_default();
            kept.iter().position(|id| {
                state.get(id).is_some_and(|record| {
                    gids(record.get("optionValues")).iter().any(|v| v == value)
                })
            })
        });
        if let Some(record) = state.get_mut(&variant) {
            record.insert("selectedOptions".into(), json!(selected));
        }
    }
}

/// `combinedListingUpdate`.
fn combined_listing_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"product": null});
    let state = mutation.state();
    let parent = ctx
        .arg_str("parentProductId")
        .unwrap_or_default()
        .to_string();
    if !state.is_of(&parent, "Product") {
        return Ok(payload(
            empty,
            &[error(&["parentProductId"], "PARENT_PRODUCT_NOT_FOUND")],
        ));
    }
    match role(state, &parent).as_deref() {
        Some("PARENT") => {}
        Some("CHILD") => {
            let error = error(
                &["parentProductId"],
                "PARENT_PRODUCT_CANNOT_BE_COMBINED_LISTING_CHILD",
            );
            return Ok(payload(empty, &[error]));
        }
        _ => {
            let error = error(
                &["parentProductId"],
                "PARENT_PRODUCT_MUST_BE_A_COMBINED_LISTING",
            );
            return Ok(payload(empty, &[error]));
        }
    }
    let added = super::records::arg_objects(ctx, "productsAdded");
    let edited = super::records::arg_objects(ctx, "productsEdited");
    let removed = gids(ctx.arg("productsRemovedIds"));
    let order = super::records::arg_objects(ctx, "optionsAndValues");
    let mut errors = Vec::new();
    if ctx
        .arg_str("title")
        .is_some_and(|title| title.chars().count() > TITLE_LENGTH)
    {
        errors.push(error(&["title"], "TITLE_TOO_LONG"));
    }
    let current = children(state, &parent);
    let is_child = |product: &str| current.iter().any(|(child, _)| child == product);
    let mut names = names_in_use(state, &parent);
    // The combinations the listing will hold, to refuse two children of the same values.
    let mut combinations: Vec<(String, Vec<(String, String)>)> = current
        .iter()
        .filter(|(child, _)| !removed.contains(child))
        .map(|(child, variant)| (child.clone(), combination_of(state, variant)))
        .collect();
    for (index, item) in edited.iter().enumerate() {
        let at = index.to_string();
        let child = text(item, "childProductId").unwrap_or_default();
        let path = ["productsEdited", at.as_str()];
        if removed.iter().any(|id| id == child) {
            errors.push(error(&path, "EDIT_AND_REMOVE_ON_SAME_PRODUCTS"));
            continue;
        }
        if !is_child(child) {
            errors.push(error(&path, "PRODUCT_MEMBERSHIP_NOT_FOUND"));
            continue;
        }
        let picked = selections(item);
        check_selections(&picked, &path, &mut names, &mut errors);
        if let Some(entry) = combinations.iter_mut().find(|(known, _)| known == child) {
            entry.1 = combination(&picked);
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    for (index, item) in added.iter().enumerate() {
        let at = index.to_string();
        let path = ["productsAdded", at.as_str()];
        let child = text(item, "childProductId").unwrap_or_default();
        let code = if !state.is_of(child, "Product") {
            Some("PRODUCT_NOT_FOUND")
        } else if child == parent || role(state, child).as_deref() == Some("PARENT") {
            Some("CANNOT_HAVE_PARENT_AS_CHILD")
        } else if role(state, child).as_deref() == Some("CHILD") {
            Some("PRODUCT_IS_ALREADY_A_CHILD")
        } else if seen.contains(&child) {
            Some("CANNOT_HAVE_DUPLICATED_PRODUCTS")
        } else {
            None
        };
        seen.push(child);
        if let Some(code) = code {
            errors.push(error(&path, code));
            continue;
        }
        let picked = selections(item);
        check_selections(&picked, &path, &mut names, &mut errors);
        combinations.push((child.to_string(), combination(&picked)));
    }
    for (index, child) in removed.iter().enumerate() {
        if !is_child(child) {
            errors.push(error(
                &["productsRemovedIds", &index.to_string()],
                "PRODUCT_MEMBERSHIP_NOT_FOUND",
            ));
        }
    }
    let mut sorted: Vec<&Vec<(String, String)>> = combinations.iter().map(|(_, c)| c).collect();
    let count = sorted.len();
    sorted.sort();
    sorted.dedup();
    if sorted.len() != count {
        errors.push(error(
            &["productsAdded"],
            "CANT_ADD_OPTIONS_VALUES_IF_ALREADY_EXISTS",
        ));
    }
    if combinations.len() > MAX_CHILDREN {
        errors.push(error(&["productsAdded"], "TOO_MANY_PRODUCTS"));
    }
    for (index, option) in order.iter().enumerate() {
        let at = index.to_string();
        let path = ["optionsAndValues", at.as_str()];
        let values: Vec<String> = option
            .get("values")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_lowercase)
            .collect();
        if text(option, "name").is_none_or(|name| name.trim().is_empty()) {
            errors.push(error(&path, "OPTION_NAME_CANNOT_BE_BLANK"));
        }
        if values.is_empty() {
            errors.push(error(&path, "OPTION_VALUES_CANNOT_BE_EMPTY"));
        }
        let mut unique = values.clone();
        unique.sort();
        unique.dedup();
        if unique.len() != values.len() {
            errors.push(error(&path, "CANNOT_HAVE_REPEATED_OPTION_VALUES"));
        }
        let name = text(option, "name").unwrap_or_default().to_lowercase();
        let missing = combinations.iter().any(|(_, combination)| {
            combination
                .iter()
                .any(|(option, value)| *option == name && !values.contains(value))
        });
        if missing {
            errors.push(error(&path, "OPTION_VALUES_MUST_BE_COMPLETE"));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }

    let state = mutation.state_mut();
    let mut changed_roles = Vec::new();
    // A parent made by `productCreate` has its default variant until it has children.
    if current.is_empty() && !added.is_empty() {
        let defaults = gids(state.get(&parent).and_then(|record| record.get("variants")));
        for variant in defaults {
            variants::remove_variant(state, &parent, &variant);
        }
    }
    let mut members = current.clone();
    for child in &removed {
        if let Some((_, variant)) = members.iter().find(|(known, _)| known == child).cloned() {
            variants::remove_variant(state, &parent, &variant);
        }
        members.retain(|(known, _)| known != child);
        if let Some(record) = state.get_mut(child) {
            record.insert("combinedListingRole".into(), Value::Null);
            record.remove(PARENT);
        }
        changed_roles.push(child.clone());
    }
    for item in &edited {
        let child = text(item, "childProductId").unwrap_or_default();
        if let Some((_, variant)) = members.iter().find(|(known, _)| known == child) {
            let picked = selections(item);
            let choices = choices_for(state, &parent, &picked);
            variants::set_choices(state, variant, &choices);
        }
    }
    let stock: variants::Stock = Vec::new();
    for item in &added {
        let child = text(item, "childProductId").unwrap_or_default().to_string();
        let picked = selections(item);
        let choices = choices_for(state, &parent, &picked);
        let variant = variants::insert_variant(state, &parent, &choices, &Map::new(), &stock);
        let price = cheapest(state, &child);
        if let Some(record) = state.get_mut(&variant) {
            record.insert("price".into(), json!(value::money(price)));
        }
        if let Some(record) = state.get_mut(&child) {
            record.insert("combinedListingRole".into(), json!("CHILD"));
            record.insert(PARENT.into(), json!(parent));
        }
        members.push((child.clone(), variant));
        changed_roles.push(child);
    }
    tidy(state, &parent, &order);
    if let Some(record) = state.get_mut(&parent) {
        record.insert(
            CHILDREN.into(),
            json!(
                members
                    .iter()
                    .map(|(child, variant)| json!({"product": child, "parentVariant": variant}))
                    .collect::<Vec<_>>()
            ),
        );
        if let Some(title) = ctx.arg_str("title") {
            record.insert("title".into(), json!(title.trim()));
        }
    }
    state.touch(&parent);
    product_changed(state, &parent);
    state.emit("products/update", &parent);
    for child in &changed_roles {
        state.touch(child);
        state.emit("products/update", child);
    }
    Ok(payload(json!({"product": parent}), &[]))
}

/// `Product.combinedListing`: the listing of a parent, or of a child's parent.
fn combined_listing(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let product = object.key().unwrap_or_default();
    let parent = match role(state, product).as_deref() {
        Some("PARENT") => product.to_string(),
        Some("CHILD") => match state.get(product).and_then(|record| field(record, PARENT)) {
            Some(parent) => parent.to_string(),
            None => return Ok(Output::Null),
        },
        _ => return Ok(Output::Null),
    };
    let listing = json!({
        "parentProduct": parent,
        "combinedListingChildren": children(state, &parent)
            .iter()
            .map(|(child, variant)| json!({"product": child, "parentVariant": variant}))
            .collect::<Vec<_>>(),
    });
    r.convert(ctx, &listing, false)
}
