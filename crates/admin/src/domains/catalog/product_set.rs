//! `productSet`: the whole state of a product in one call, what integrations that sync a catalog
//! from another system use.
//!
//! As shopify.dev describes it (the reference of 2026-10 and "Sync product data from an external
//! source"):
//!
//! - The product is the one of `identifier` (`id`, `handle`, `customId`), else of the deprecated
//!   `input.id`; without one, a new product. A `handle` or a `customId` that names no product
//!   creates one with that handle or that metafield (an upsert); an `id` that names none is
//!   "Product does not exist" (`PRODUCT_DOES_NOT_EXIST`).
//! - Fields given are set, fields left out stay as they are ("For other product fields (like
//!   `description` or `tags`), omitted fields remain unchanged while included fields are updated
//!   to match your input, even if you provide an empty value").
//! - "For options and variants, the mutation replaces the complete state—any options or variants
//!   not included in your input are removed." An option is matched by `id`, else by name; a value
//!   by `id`, else by name; a variant by `id` ("Specifies the product variant to update or create
//!   a new variant if absent"). `productOptions: []` with `variants: []` leaves the product with
//!   its default variant (the reference's example). A new product needs both or neither
//!   (`PRODUCT_OPTIONS_INPUT_MISSING`, `VARIANTS_INPUT_MISSING`).
//! - `inventoryQuantities` set the `available` (or `on_hand`) quantity at a location: for a new
//!   variant, the locations it is stocked at; for an existing one, only where it is stocked
//!   already ("If you're updating an existing variant, then you can only update the quantities at
//!   locations where the variant is already stocked").
//! - `collections` is the complete list of the custom collections the product is in. `files`
//!   (each a `FileSetInput`) is the complete list of its media: those named by `id` stay, those
//!   with an `originalSource` are created. `metafields` are set (created or changed); lsf does not
//!   remove the product's other metafields (the guide documents the replacement for options and
//!   variants only).
//! - `synchronous: false` answers a `ProductSetOperation` instead of the product (Shopify's is
//!   `CREATED` then runs in the background); lsf's is `COMPLETE` at once, with its `product`.
//! - Events: `products/create` or `products/update`, and the inventory events of the variants
//!   created (`inventory_items/create`, `inventory_levels/connect`), deleted
//!   (`inventory_items/delete`) and stocked (`inventory_levels/update`).

use serde_json::{Map, Value, json};

use super::records::{self, coded, default_location, field, input, is, item_of, objects, text};
use super::variants::{self, Choice, ProductOption, Reader};
use super::{gids, media, product_changed, products};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry.mutation("productSet", product_set);
}

/// What `productSet` does to the product.
enum Target {
    Existing(String),
    /// A new product, with the handle or the `customId` metafield of the identifier.
    New {
        handle: Option<String>,
        custom_id: Option<Value>,
    },
}

/// An option as `productSet` will leave it: the existing option it is (if any), its name, its
/// values (the existing value each is, if any, and its name).
struct PlannedOption {
    id: Option<String>,
    name: String,
    values: Vec<(Option<String>, String)>,
}

/// A variant as `productSet` will leave it.
struct PlannedVariant<'i> {
    id: Option<String>,
    /// `(option index, value name)`.
    choices: Vec<(usize, String)>,
    input: &'i Map<String, Value>,
    position: usize,
}

fn error(path: &[&str], message: &str, code: &str) -> UserError {
    let mut field: Vec<&str> = vec!["input"];
    field.extend(path);
    coded(&field, message, code)
}

/// `productSet`.
fn product_set(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"product": null, "productSetOperation": null});
    let input = input(ctx, "input");
    let state = mutation.state();
    let target = match target(state, ctx, &input) {
        Ok(target) => target,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let existing = match &target {
        Target::Existing(product) => Some(product.clone()),
        Target::New { .. } => None,
    };
    let mut errors = Vec::new();
    // The plain fields.
    let title = text(&input, "title").map(str::trim);
    match (title, &existing) {
        (Some(title), _) => products::check_title(title, &mut errors),
        (None, None) => products::check_title("", &mut errors),
        _ => {}
    }
    for error in &mut errors {
        error.field = Some(vec!["input".into(), "title".into()]);
        error.code = Some("INVALID_PRODUCT".into());
    }
    let handle = text(&input, "handle")
        .map(lsf_core::util::handleize)
        .or_else(|| match &target {
            Target::New { handle, .. } => handle.clone(),
            Target::Existing(_) => None,
        });
    if let Some(handle) = &handle
        && products::handle_taken(state, handle, existing.as_deref())
    {
        errors.push(error(
            &["handle"],
            &format!("Handle '{handle}' already in use. Please provide a new handle."),
            "HANDLE_NOT_UNIQUE",
        ));
    }
    let collections = gids(input.get("collections"));
    for (index, collection) in collections.iter().enumerate() {
        if !is(state, collection, "Collection") {
            errors.push(error(
                &["collections", &index.to_string()],
                &format!("Collection {collection} does not exist"),
                "INVALID_INPUT",
            ));
        }
    }
    let metafields = objects(&input, "metafields");
    for mut found in records::check_metafields(
        state,
        existing.as_deref(),
        "PRODUCT",
        &metafields,
        &["input", "metafields"],
        None,
    ) {
        found.code = Some("INVALID_METAFIELD".into());
        errors.push(found);
    }
    // Options and variants.
    let current = existing
        .as_ref()
        .map(|product| variants::options_of(state, product))
        .unwrap_or_default();
    let option_inputs = input
        .get("productOptions")
        .map(|_| objects(&input, "productOptions"));
    let variant_inputs = input.get("variants").map(|_| objects(&input, "variants"));
    if existing.is_none() {
        match (&option_inputs, &variant_inputs) {
            (Some(options), None) if !options.is_empty() => errors.push(error(
                &["variants"],
                "Variants input must be provided when product options are provided.",
                "VARIANTS_INPUT_MISSING",
            )),
            (None, Some(variants)) if !variants.is_empty() => errors.push(error(
                &["productOptions"],
                "Product options input must be provided when variants are provided.",
                "PRODUCT_OPTIONS_INPUT_MISSING",
            )),
            _ => {}
        }
    }
    let planned_options = match &option_inputs {
        Some(options) => plan_options(&current, options, &mut errors),
        None => current
            .iter()
            .map(|option| PlannedOption {
                id: Some(option.id.clone()),
                name: option.name.clone(),
                values: option
                    .values
                    .iter()
                    .map(|(id, name)| (Some(id.clone()), name.clone()))
                    .collect(),
            })
            .collect(),
    };
    let own_variants = existing
        .as_ref()
        .and_then(|product| state.get(product))
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default();
    let planned_variants = match &variant_inputs {
        Some(inputs) => plan_variants(state, &planned_options, inputs, &own_variants, &mut errors),
        None => Vec::new(),
    };
    for (index, variant) in variant_inputs.iter().flatten().enumerate() {
        let index = index.to_string();
        let own = text(variant, "id").filter(|id| own_variants.iter().any(|own| own == id));
        for mut found in records::check_metafields(
            state,
            own,
            "PRODUCTVARIANT",
            &objects(variant, "metafields"),
            &["input", "variants", &index, "metafields"],
            None,
        ) {
            found.code = Some("INVALID_METAFIELD".into());
            errors.push(found);
        }
    }
    if option_inputs.is_some() && variant_inputs.is_none() && existing.is_some() {
        errors.push(error(
            &["variants"],
            "Variants input must be provided when product options are provided.",
            "VARIANTS_INPUT_MISSING",
        ));
    }
    let files = input.get("files").map(|_| objects(&input, "files"));
    if let Some(files) = &files {
        for (index, file) in files.iter().enumerate() {
            let source = text(file, "originalSource").unwrap_or_default();
            let known = text(file, "id").is_some_and(|id| media::is_media(state, id));
            if !known && !(source.starts_with("https://") || source.starts_with("http://")) {
                errors.push(error(
                    &["files", &index.to_string(), "originalSource"],
                    "File URL is invalid",
                    "INVALID_INPUT",
                ));
            }
        }
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    if existing.is_some()
        && input
            .get("combinedListingRole")
            .is_some_and(|role| !role.is_null())
    {
        mutation.note("productSet: `combinedListingRole` can only be set when a product is created: it was ignored");
    }
    for name in ["category", "claimOwnership", "redirectNewHandle"] {
        if input.get(name).is_some_and(|value| !value.is_null()) {
            mutation.note(format!(
                "productSet: lsf does not simulate `{name}` yet: it was ignored"
            ));
        }
    }

    let state = mutation.state_mut();
    let before = existing
        .as_ref()
        .map(|product| records::stock_of(state, std::slice::from_ref(product)))
        .unwrap_or_default();
    let created = existing.is_none();
    let product = match existing {
        Some(product) => product,
        None => {
            let title = title.unwrap_or_default().to_string();
            let handle = handle
                .clone()
                .unwrap_or_else(|| products::free_handle(state, &title));
            new_product(state, &title, &handle)
        }
    };
    let mut events: Vec<(&str, String)> = Vec::new();
    // The plain fields.
    let origin = records::origin(state);
    if let Some(record) = state.get_mut(&product) {
        products::apply_fields(record, &input);
        if let Some(gift_card) = input.get("giftCard").and_then(Value::as_bool) {
            record.insert("isGiftCard".into(), json!(gift_card));
        }
        if created && let Some(role) = text(&input, "combinedListingRole") {
            record.insert("combinedListingRole".into(), json!(role));
        }
        if let Some(handle) = &handle {
            record.insert("handle".into(), json!(handle));
            record.insert(
                "onlineStorePreviewUrl".into(),
                json!(format!("{origin}/products/{handle}")),
            );
        }
    }
    if let Target::New {
        custom_id: Some(custom),
        ..
    } = &target
    {
        let metafield = value::object(json!({
            "namespace": custom.get("namespace").cloned().unwrap_or(json!("$app")),
            "key": custom.get("key"),
            "type": "id",
            "value": custom.get("value"),
        }));
        records::set_metafields(state, &product, "PRODUCT", &[&metafield]);
    }
    // Media.
    if let Some(files) = &files {
        let keep: Vec<String> = files
            .iter()
            .filter_map(|file| text(file, "id"))
            .map(str::to_string)
            .collect();
        let current_media = state
            .get(&product)
            .map(|record| gids(record.get("media")))
            .unwrap_or_default();
        let gone: Vec<String> = current_media
            .into_iter()
            .filter(|medium| !keep.contains(medium))
            .collect();
        media::remove_media(state, &product, &gone);
        for file in files {
            if text(file, "id").is_some() {
                continue;
            }
            let kind = match text(file, "contentType") {
                Some("VIDEO") => "VIDEO",
                Some("EXTERNAL_VIDEO") => "EXTERNAL_VIDEO",
                Some("MODEL_3D") => "MODEL_3D",
                _ => "IMAGE",
            };
            let media_input = value::object(json!({
                "originalSource": file.get("originalSource"),
                "alt": file.get("alt"),
                "mediaContentType": kind,
            }));
            media::create_media(state, &product, &media_input);
        }
    }
    // Options and variants.
    if option_inputs.is_some() || variant_inputs.is_some() || created {
        let option_ids = apply_options(state, &product, &planned_options, &current);
        let mut keep = Vec::new();
        let mut ordered: Vec<(usize, String)> = Vec::new();
        let options = variants::options_of(state, &product);
        for planned in &planned_variants {
            let default_choice = planned.choices.is_empty().then(|| {
                options
                    .iter()
                    .find(|option| option.name == "Title")
                    .map(|title| Choice {
                        option: title.id.clone(),
                        name: "Default Title".into(),
                        value: title.values.first().map(|(id, _)| id.clone()),
                    })
            });
            let choices: Vec<Choice> = planned
                .choices
                .iter()
                .filter_map(|(index, name)| {
                    let option = options
                        .iter()
                        .find(|option| Some(&option.id) == option_ids.get(*index))?;
                    let value = option
                        .values
                        .iter()
                        .find(|(_, known)| known == name)
                        .map(|(id, _)| id.clone());
                    Some(Choice {
                        option: option.id.clone(),
                        name: name.clone(),
                        value,
                    })
                })
                .collect();
            let choices = match default_choice.flatten() {
                Some(choice) => vec![choice],
                None => choices,
            };
            let choices = variants::ensure_values(state, &choices);
            let variant = match &planned.id {
                Some(variant) => {
                    variants::set_choices(state, variant, &choices);
                    if variants::apply_fields(state, variant, planned.input)
                        && let Some(item) = item_of(state, variant)
                    {
                        state.touch(&item);
                        events.push(("inventory_items/update", item));
                    }
                    set_quantities(state, variant, planned.input, &mut events);
                    let metafields = objects(planned.input, "metafields");
                    records::set_metafields(state, variant, "PRODUCTVARIANT", &metafields);
                    state.touch(variant);
                    variant.clone()
                }
                None => {
                    let stock = new_stock(state, planned.input);
                    let variant =
                        variants::insert_variant(state, &product, &choices, planned.input, &stock);
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
                    variant
                }
            };
            if let Some(file) = planned.input.get("file").and_then(Value::as_object)
                && let Some(source) = text(file, "originalSource")
            {
                let medium = state
                    .get(&product)
                    .map(|record| gids(record.get("media")))
                    .unwrap_or_default()
                    .into_iter()
                    .find(|medium| {
                        state.get(medium).is_some_and(|record| {
                            record
                                .get("originalSource")
                                .and_then(|original| original.get("url"))
                                .and_then(Value::as_str)
                                == Some(source)
                        })
                    });
                if let Some(medium) = medium {
                    media::attach_to_variant(state, &variant, &[medium]);
                }
            }
            keep.push(variant.clone());
            ordered.push((planned.position, variant));
        }
        if variant_inputs.is_some() || created {
            // The variants not listed go.
            let current_variants = state
                .get(&product)
                .map(|record| gids(record.get("variants")))
                .unwrap_or_default();
            for variant in current_variants
                .iter()
                .filter(|variant| !keep.contains(variant))
            {
                variants::remove_variant(state, &product, variant);
            }
            if keep.is_empty() {
                // No variant: the default one, on the option "Title".
                reset_to_default(state, &product);
            } else {
                ordered.sort_by_key(|(position, _)| *position);
                let order: Vec<&String> = ordered.iter().map(|(_, variant)| variant).collect();
                if let Some(record) = state.get_mut(&product) {
                    record.insert("variants".into(), json!(order));
                }
            }
        }
    }
    if input.contains_key("collections") {
        let current: Vec<String> = state
            .get(&product)
            .map(|record| gids(record.get("collections")))
            .unwrap_or_default()
            .into_iter()
            .filter(|collection| {
                state
                    .get(collection)
                    .is_some_and(|record| !super::collections::is_smart(record))
            })
            .collect();
        let leave: Vec<String> = current
            .into_iter()
            .filter(|collection| !collections.contains(collection))
            .collect();
        products::join_collections(state, &product, &collections, &leave);
    }
    records::set_metafields(state, &product, "PRODUCT", &metafields);
    product_changed(state, &product);
    records::after_inventory_change(state, std::slice::from_ref(&product), &before);
    if !created {
        state.touch(&product);
    }
    let topic = if created {
        "products/create"
    } else {
        "products/update"
    };
    state.emit(topic, &product);
    for (topic, gid) in &events {
        state.emit(topic, gid);
    }
    if ctx.arg_bool("synchronous") == Some(false) {
        let operation = state.new_gid("ProductSetOperation");
        state.insert(value::object(json!({
            "__typename": "ProductSetOperation",
            "id": operation,
            "product": product,
            "status": "COMPLETE",
            "userErrors": [],
        })));
        return Ok(payload(
            json!({"product": null, "productSetOperation": operation}),
            &[],
        ));
    }
    Ok(payload(
        json!({"product": product, "productSetOperation": null}),
        &[],
    ))
}

/// The product `productSet` works on.
fn target(
    state: &ShopState,
    ctx: &FieldContext<'_>,
    input: &Map<String, Value>,
) -> Result<Target, UserError> {
    let identifier = ctx.arg("identifier").and_then(Value::as_object);
    let id = identifier
        .and_then(|identifier| text(identifier, "id"))
        .or_else(|| text(input, "id"));
    if let Some(id) = id {
        return if is(state, id, "Product") {
            Ok(Target::Existing(id.to_string()))
        } else {
            Err(coded(
                &["input", "id"],
                "Product does not exist",
                "PRODUCT_DOES_NOT_EXIST",
            ))
        };
    }
    if let Some(handle) = identifier.and_then(|identifier| text(identifier, "handle")) {
        let found = state.keys("Product").find(|product| {
            state
                .get(product)
                .and_then(|record| field(record, "handle"))
                == Some(handle)
        });
        return Ok(match found {
            Some(product) => Target::Existing(product.to_string()),
            None => Target::New {
                handle: Some(lsf_core::util::handleize(handle)),
                custom_id: None,
            },
        });
    }
    if let Some(custom) = identifier.and_then(|identifier| identifier.get("customId")) {
        return Ok(match products::product_by_custom_id(state, custom) {
            Some(product) => Target::Existing(product),
            None => Target::New {
                handle: None,
                custom_id: Some(custom.clone()),
            },
        });
    }
    Ok(Target::New {
        handle: None,
        custom_id: None,
    })
}

/// A new product without options or variants yet: `productSet` gives them.
fn new_product(state: &mut ShopState, title: &str, handle: &str) -> String {
    let now = state.timestamp();
    let vendor = state
        .shop()
        .and_then(|shop| field(shop, "name"))
        .unwrap_or_default()
        .to_string();
    let origin = records::origin(state);
    let product = state.new_gid("Product");
    state.insert(value::object(json!({
        "__typename": "Product",
        "id": product,
        "title": title,
        "handle": handle,
        "descriptionHtml": "",
        "bodyHtml": "",
        "description": "",
        "vendor": vendor,
        "productType": "",
        "tags": [],
        "status": "ACTIVE",
        "createdAt": now,
        "updatedAt": now,
        "publishedAt": null,
        "templateSuffix": null,
        "giftCardTemplateSuffix": null,
        "isGiftCard": false,
        "requiresSellingPlan": false,
        "hasVariantsThatRequiresComponents": false,
        "options": [],
        "variants": [],
        "media": [],
        "images": [],
        "seo": {"title": null, "description": null},
        "onlineStoreUrl": null,
        "onlineStorePreviewUrl": format!("{origin}/products/{handle}"),
        "collections": [],
        "sellingPlanGroups": [],
        "metafields": [],
    })));
    product
}

/// The options of `productOptions`, matched with the product's.
fn plan_options(
    current: &[ProductOption],
    inputs: &[&Map<String, Value>],
    errors: &mut Vec<UserError>,
) -> Vec<PlannedOption> {
    if inputs.len() > 3 {
        errors.push(error(
            &["productOptions"],
            "Can only specify a maximum of 3 options",
            "OPTIONS_OVER_LIMIT",
        ));
    }
    let mut planned: Vec<PlannedOption> = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let at = index.to_string();
        let existing = match (text(input, "id"), text(input, "name")) {
            (Some(id), _) => match current.iter().find(|option| option.id == id) {
                Some(option) => Some(option),
                None => {
                    errors.push(error(
                        &["productOptions", &at, "id"],
                        "Option does not exist",
                        "OPTION_DOES_NOT_EXIST",
                    ));
                    continue;
                }
            },
            (None, Some(name)) => current.iter().find(|option| option.name == name.trim()),
            (None, None) => None,
        };
        let name = text(input, "name")
            .map(|name| name.trim().to_string())
            .or_else(|| existing.map(|option| option.name.clone()))
            .unwrap_or_default();
        if name.is_empty() {
            errors.push(error(
                &["productOptions", &at, "name"],
                "Option name can't be blank",
                "INVALID_INPUT",
            ));
        }
        if planned
            .iter()
            .any(|other| other.name.eq_ignore_ascii_case(&name))
        {
            errors.push(error(
                &["productOptions", &at, "name"],
                &format!("Option '{name}' already exists."),
                "DUPLICATED_OPTION_NAME",
            ));
        }
        let value_inputs = objects(input, "values");
        let mut values: Vec<(Option<String>, String)> = Vec::new();
        for (position, value) in value_inputs.iter().enumerate() {
            let known = match (text(value, "id"), text(value, "name")) {
                (Some(id), _) => match existing
                    .and_then(|option| option.values.iter().find(|(known, _)| known == id))
                {
                    Some(found) => Some(found.clone()),
                    None => {
                        errors.push(error(
                            &["productOptions", &at, "values", &position.to_string(), "id"],
                            "Option value does not exist",
                            "OPTION_VALUE_DOES_NOT_EXIST",
                        ));
                        continue;
                    }
                },
                (None, Some(name)) => existing
                    .and_then(|option| option.values.iter().find(|(_, known)| known == name.trim()))
                    .cloned(),
                _ => None,
            };
            let value_name = text(value, "name")
                .map(|name| name.trim().to_string())
                .or_else(|| known.as_ref().map(|(_, name)| name.clone()))
                .unwrap_or_default();
            if value_name.is_empty() {
                errors.push(error(
                    &[
                        "productOptions",
                        &at,
                        "values",
                        &position.to_string(),
                        "name",
                    ],
                    "Option value name can't be blank",
                    "INVALID_INPUT",
                ));
                continue;
            }
            if values
                .iter()
                .any(|(_, other)| other.eq_ignore_ascii_case(&value_name))
            {
                errors.push(error(
                    &[
                        "productOptions",
                        &at,
                        "values",
                        &position.to_string(),
                        "name",
                    ],
                    "Duplicated option value.",
                    "DUPLICATED_OPTION_VALUE",
                ));
                continue;
            }
            values.push((known.map(|(id, _)| id), value_name));
        }
        if values.is_empty() {
            errors.push(error(
                &["productOptions", &at, "values"],
                "Each option must have at least one option value specified.",
                "OPTION_VALUES_MISSING",
            ));
        }
        planned.push(PlannedOption {
            id: existing.map(|option| option.id.clone()),
            name,
            values,
        });
    }
    planned
}

/// The variants of `variants`, read against the planned options.
fn plan_variants<'i>(
    state: &ShopState,
    options: &[PlannedOption],
    inputs: &[&'i Map<String, Value>],
    own: &[String],
    errors: &mut Vec<UserError>,
) -> Vec<PlannedVariant<'i>> {
    // The planned options as `ProductOption`s, with the names as ids for the new ones.
    let as_options: Vec<ProductOption> = options
        .iter()
        .enumerate()
        .map(|(index, option)| ProductOption {
            id: option
                .id
                .clone()
                .unwrap_or_else(|| format!("new-option-{index}")),
            name: option.name.clone(),
            values: option
                .values
                .iter()
                .enumerate()
                .map(|(position, (id, name))| {
                    (
                        id.clone()
                            .unwrap_or_else(|| format!("new-value-{index}-{position}")),
                        name.clone(),
                    )
                })
                .collect(),
        })
        .collect();
    let mut planned = Vec::new();
    let mut taken: Vec<Vec<String>> = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let path = vec![
            "input".to_string(),
            "variants".to_string(),
            index.to_string(),
        ];
        let id = text(input, "id").map(str::to_string);
        if let Some(id) = &id
            && !own.contains(id)
        {
            errors.push(UserError {
                field: Some(path.iter().cloned().chain(["id".to_string()]).collect()),
                message: "Product variant does not exist".into(),
                code: Some(
                    if is(state, id, "ProductVariant") {
                        "INVALID_VARIANT"
                    } else {
                        "PRODUCT_VARIANT_DOES_NOT_EXIST"
                    }
                    .into(),
                ),
            });
        }
        let mut field_errors = Vec::new();
        variants::check_fields(input, &path, &mut field_errors);
        for mut found in field_errors {
            found.code = Some("INVALID_VARIANT".into());
            errors.push(found);
        }
        if options.is_empty() {
            // The product will have the option "Title" only.
            planned.push(PlannedVariant {
                id,
                choices: Vec::new(),
                input,
                position: input
                    .get("position")
                    .and_then(Value::as_i64)
                    .map_or(index + 1, |p| p.max(1) as usize),
            });
            continue;
        }
        let values = objects(input, "optionValues");
        match variants::choices(&as_options, &values, &path, Reader::Set) {
            Ok(choices) => {
                let key = variants::combination(&choices);
                if taken.contains(&key) {
                    errors.push(UserError {
                        field: Some(path.clone()),
                        message: format!(
                            "The variant '{}' already exists.",
                            variants::title_of(&choices)
                        ),
                        code: Some("INVALID_VARIANT".into()),
                    });
                }
                taken.push(key);
                let choices = choices
                    .iter()
                    .filter_map(|choice| {
                        let option = as_options
                            .iter()
                            .position(|option| option.id == choice.option)?;
                        Some((option, choice.name.clone()))
                    })
                    .collect();
                planned.push(PlannedVariant {
                    id,
                    choices,
                    input,
                    position: input
                        .get("position")
                        .and_then(Value::as_i64)
                        .map_or(index + 1, |p| p.max(1) as usize),
                });
            }
            Err(found) => errors.extend(found),
        }
    }
    planned
}

/// Writes the planned options: the existing ones changed, the new ones created, the others
/// removed. Returns the options' GIDs in order.
fn apply_options(
    state: &mut ShopState,
    product: &str,
    planned: &[PlannedOption],
    current: &[ProductOption],
) -> Vec<String> {
    let now = state.timestamp();
    let mut ids = Vec::new();
    for (index, option) in planned.iter().enumerate() {
        let id = match &option.id {
            Some(id) => id.clone(),
            None => {
                let id = state.new_gid("ProductOption");
                state.insert(value::object(json!({
                    "__typename": "ProductOption",
                    "id": id,
                    "name": option.name,
                    "position": index + 1,
                    "values": [],
                    "optionValues": [],
                    "createdAt": now,
                })));
                id
            }
        };
        let mut value_ids = Vec::new();
        for (value, name) in &option.values {
            let value_id = match value {
                Some(value) => {
                    if let Some(record) = state.get_mut(value) {
                        record.insert("name".into(), json!(name));
                    }
                    value.clone()
                }
                None => {
                    let value = state.new_gid("ProductOptionValue");
                    state.insert(value::object(json!({
                        "__typename": "ProductOptionValue",
                        "id": value,
                        "name": name,
                        "hasVariants": false,
                        "swatch": null,
                    })));
                    value
                }
            };
            value_ids.push(value_id);
        }
        // The values not listed go (`productOptions` is the complete list).
        if let Some(existing) = current.iter().find(|known| known.id == id) {
            for (value, _) in &existing.values {
                if !value_ids.contains(value) {
                    state.remove(value);
                }
            }
        }
        let names: Vec<&String> = option.values.iter().map(|(_, name)| name).collect();
        if let Some(record) = state.get_mut(&id) {
            record.insert("name".into(), json!(option.name));
            record.insert("position".into(), json!(index + 1));
            record.insert("optionValues".into(), json!(value_ids));
            record.insert("values".into(), json!(names));
        }
        ids.push(id);
    }
    for option in current {
        if !ids.contains(&option.id) {
            for (value, _) in &option.values {
                state.remove(value);
            }
            state.remove(&option.id);
        }
    }
    if ids.is_empty() {
        // Without options, the option "Title" of the default variant.
        let id = state.new_gid("ProductOption");
        state.insert(value::object(json!({
            "__typename": "ProductOption",
            "id": id,
            "name": "Title",
            "position": 1,
            "values": [],
            "optionValues": [],
            "createdAt": now,
        })));
        variants::add_value(state, &id, "Default Title");
        ids.push(id);
    }
    if let Some(record) = state.get_mut(product) {
        record.insert("options".into(), json!(ids));
    }
    ids
}

/// Leaves a product with one variant, "Default Title", on its option "Title".
fn reset_to_default(state: &mut ShopState, product: &str) {
    let options = variants::options_of(state, product);
    let Some(title) = options.iter().find(|option| option.name == "Title") else {
        return;
    };
    let choices = vec![Choice {
        option: title.id.clone(),
        name: "Default Title".into(),
        value: title.values.first().map(|(id, _)| id.clone()),
    }];
    let choices = variants::ensure_values(state, &choices);
    let stock = default_location(state)
        .map(|location| vec![(location, 0)])
        .unwrap_or_default();
    let variant = variants::insert_variant(state, product, &choices, &Map::new(), &stock);
    products::emit_new_inventory(state, &variant);
}

/// How a new variant of `productSet` is stocked: its `inventoryQuantities`, else the first
/// active location with 0.
fn new_stock(state: &ShopState, input: &Map<String, Value>) -> variants::Stock {
    let quantities = objects(input, "inventoryQuantities");
    if quantities.is_empty() {
        return default_location(state)
            .map(|location| vec![(location, 0)])
            .unwrap_or_default();
    }
    let mut stock: variants::Stock = Vec::new();
    for quantity in quantities {
        let location = text(quantity, "locationId").unwrap_or_default().to_string();
        if !is(state, &location, "Location") {
            continue;
        }
        let amount = quantity
            .get("quantity")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        match stock.iter_mut().find(|(known, _)| *known == location) {
            Some(entry) => entry.1 = amount,
            None => stock.push((location, amount)),
        }
    }
    stock
}

/// Sets the `inventoryQuantities` of an existing variant, at the locations it is stocked at.
fn set_quantities(
    state: &mut ShopState,
    variant: &str,
    input: &Map<String, Value>,
    events: &mut Vec<(&'static str, String)>,
) {
    let Some(item) = item_of(state, variant) else {
        return;
    };
    for quantity in objects(input, "inventoryQuantities") {
        let location = text(quantity, "locationId").unwrap_or_default();
        let name = text(quantity, "name").unwrap_or("available");
        let wanted = quantity
            .get("quantity")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let Some(level) = records::level_at(state, &item, location) else {
            continue;
        };
        let current = state
            .get(&level)
            .map_or(0, |record| records::quantity(record, name));
        if wanted != current {
            records::change_quantity(state, &level, name, wanted - current);
            events.push(("inventory_levels/update", level));
        }
    }
}
