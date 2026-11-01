//! `productCreate`, `productUpdate`, `productDelete`: the reference mutations of the catalog,
//! the model the other mutations follow (see "Writing a resolver or a mutation" in
//! `crates/admin/docs/design.md`).
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `productCreate` creates the product with its one initial variant: "Default Title" without
//!   options, else "only one product variant is created and linked with the first option value
//!   specified for each option name". The variant costs `0.00` and its inventory is not tracked
//!   (the example response's values); its inventory item is stocked at the shop's first location
//!   (quantities 0). The product is created unpublished ("The `productCreate` mutation creates
//!   products in an unpublished state by default"), `ACTIVE` unless `status` says otherwise. A
//!   handle not given is made from the title, with a suffix when it is taken (`hat`, `hat-1`).
//!   `media` (a `CreateMediaInput` each, see [`super::media`]) and `metafields` are created with
//!   it. Events: `products/create`, `inventory_items/create`, `inventory_levels/connect`.
//! - `productUpdate` changes the fields given (`tags` replaces the tags), adds the `media`
//!   given and sets the `metafields` given: `products/update`.
//! - The `metafields` of both are checked and kept as `metafieldsSet` checks and keeps them
//!   ([`super::records::check_metafields`]): the `type` is "Required when creating or updating a
//!   metafield without a definition" (`MetafieldInput.type`), a definition's validations apply.
//! - `productDelete` removes the product, its options, variants, inventory and media, and the
//!   references of its collections: `products/delete`, `inventory_items/delete`.
//! - `productDuplicate` copies a product with its options, variants (their prices, SKUs,
//!   barcodes, inventory items, not their stock: Shopify's copy starts at 0), metafields, tags
//!   and collections; its images only with `includeImages`. The copy has `newTitle`, a handle
//!   made from it, the status `newStatus` (else the original's), and is unpublished. With
//!   `synchronous: false` it answers an operation that is already complete. Events:
//!   `products/create`, `inventory_items/create`, `inventory_levels/connect`.
//! - `productChangeStatus` (deprecated for `productUpdate`) sets the status: `products/update`.
//!
//! - `combinedListingRole` "can only be set during creation": `productCreate` keeps it (see
//!   [`super::combined_listings`]); `productUpdate` notes that it was ignored.
//!
//! The messages are Shopify's (Rails validations: "Title can't be blank"); where shopify.dev does
//! not show them, they follow the same form. Inputs lsf does not simulate yet (`category`, …)
//! are said in `extensions.lsf.notes`, never silently dropped.

use serde_json::{Map, Value, json};

use super::records::{self, item_of, objects, text};
use super::variants::{self, Choice};
use super::{gids, media, product_changed};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("productCreate", product_create)
        .mutation("productUpdate", product_update)
        .mutation("productDelete", product_delete)
        .mutation("productDuplicate", product_duplicate)
        .mutation("productChangeStatus", product_change_status);
}

/// The inputs of the product mutations that lsf does not simulate yet.
const NOT_SIMULATED: &[&str] = &[
    "category",
    "claimOwnership",
    "redirectNewHandle",
    "deleteConflictingConstrainedMetafields",
];

/// The longest title Shopify takes.
const TITLE_LENGTH: usize = 255;

/// `product` (or the deprecated `input`) of a product mutation.
fn product_input<'c>(ctx: &FieldContext<'c>) -> Map<String, Value> {
    ctx.arg("product")
        .or_else(|| ctx.arg("input"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The tags of an input: trimmed, a comma-separated item split, repeated ones (ignoring case)
/// dropped, in their order.
pub fn tags_of(value: Option<&Value>) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    let items = value.and_then(Value::as_array).cloned().unwrap_or_default();
    for item in items.iter().filter_map(Value::as_str) {
        for tag in item.split(',').map(str::trim).filter(|tag| !tag.is_empty()) {
            if !tags.iter().any(|known| known.eq_ignore_ascii_case(tag)) {
                tags.push(tag.to_string());
            }
        }
    }
    tags
}

/// Whether a product other than `except` has `handle`.
pub fn handle_taken(state: &ShopState, handle: &str, except: Option<&str>) -> bool {
    state.handle_taken("Product", handle, None, except)
}

/// A free handle made from `base`: itself, else with `-1`, `-2`, … appended.
pub fn free_handle(state: &ShopState, base: &str) -> String {
    records::free_handle(state, "Product", base, "product")
}

/// The checks of a title.
pub fn check_title(title: &str, errors: &mut Vec<UserError>) {
    if title.trim().is_empty() {
        errors.push(UserError::new(&["title"], "Title can't be blank").code("BLANK"));
    } else if title.chars().count() > TITLE_LENGTH {
        errors.push(
            UserError::new(&["title"], "Title is too long (maximum is 255 characters)")
                .code("TOO_LONG"),
        );
    }
}

/// The collections of `collectionsToJoin` (or `collectionsToLeave`), each checked to exist.
pub fn collections(
    state: &ShopState,
    input: &Map<String, Value>,
    field: &str,
    errors: &mut Vec<UserError>,
) -> Vec<String> {
    let ids = gids(input.get(field));
    for id in &ids {
        if state.type_of(id) != Some("Collection") {
            errors.push(UserError::new(
                &[field],
                format!("Collection {id} does not exist"),
            ));
        }
    }
    ids
}

/// Notes the inputs given that lsf does not simulate.
fn note_ignored(mutation: &mut Mutation<'_>, name: &str, input: &Map<String, Value>) {
    for field in NOT_SIMULATED {
        if input
            .get(*field)
            .is_some_and(|value| !value.is_null() && value != &Value::Bool(false))
        {
            mutation.note(format!(
                "{name}: lsf does not simulate `{field}` yet: it was ignored"
            ));
        }
    }
}

/// Sets the plain fields of a product from an input: those given, and only those.
pub fn apply_fields(record: &mut Record, input: &Map<String, Value>) {
    if let Some(title) = text(input, "title") {
        record.insert("title".into(), json!(title.trim()));
    }
    if let Some(html) = text(input, "descriptionHtml") {
        record.insert("descriptionHtml".into(), json!(html));
        record.insert("bodyHtml".into(), json!(html));
        record.insert("description".into(), json!(value::plain_text(html)));
    }
    for field in ["vendor", "productType"] {
        if let Some(value) = text(input, field) {
            record.insert(field.into(), json!(value.trim()));
        }
    }
    for field in ["templateSuffix", "giftCardTemplateSuffix"] {
        if let Some(value) = input.get(field) {
            let value = value.as_str().filter(|value| !value.is_empty());
            record.insert(field.into(), json!(value));
        }
    }
    if let Some(status) = text(input, "status") {
        record.insert("status".into(), json!(status));
    }
    if input.contains_key("tags") {
        record.insert("tags".into(), json!(tags_of(input.get("tags"))));
    }
    if let Some(flag) = input.get("requiresSellingPlan").and_then(Value::as_bool) {
        record.insert("requiresSellingPlan".into(), json!(flag));
    }
    if let Some(seo) = input.get("seo").and_then(Value::as_object) {
        record.insert(
            "seo".into(),
            json!({"title": seo.get("title"), "description": seo.get("description")}),
        );
    }
}

/// Adds the product to the collections (and the collections to the product), or takes it out:
/// a product already in a collection it joins keeps its place. Smart collections are left to
/// their rules.
pub fn join_collections(state: &mut ShopState, product: &str, join: &[String], leave: &[String]) {
    for collection in join.iter().chain(leave) {
        let smart = state
            .get(collection)
            .is_some_and(super::collections::is_smart);
        if !smart {
            records::link_collection(state, collection, product, join.contains(collection));
        }
    }
}

/// The options of `productOptions`, with what is wrong with them: `(name, values)`.
fn options_of(
    input: &Map<String, Value>,
    errors: &mut Vec<UserError>,
) -> Vec<(String, Vec<String>)> {
    let Some(options) = input.get("productOptions").and_then(Value::as_array) else {
        return vec![("Title".to_string(), vec!["Default Title".to_string()])];
    };
    if options.len() > 3 {
        errors.push(UserError::new(
            &["productOptions"],
            "Can only specify a maximum of 3 options",
        ));
    }
    let mut parsed = Vec::new();
    for (index, option) in options.iter().enumerate() {
        let position = index.to_string();
        let name = option
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if name.is_empty() {
            errors.push(UserError::new(
                &["productOptions", &position, "name"],
                "Option name can't be blank",
            ));
        }
        let values: Vec<String> = option
            .get("values")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| value.get("name").and_then(Value::as_str))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();
        if values.is_empty() {
            errors.push(UserError::new(
                &["productOptions", &position, "values"],
                "Option values can't be blank",
            ));
        }
        parsed.push((name.to_string(), values));
    }
    if parsed.is_empty() {
        parsed.push(("Title".to_string(), vec!["Default Title".to_string()]));
    }
    parsed
}

/// `productCreate`.
fn product_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = product_input(ctx);
    note_ignored(mutation, "productCreate", &input);
    let mut errors = Vec::new();
    let title = text(&input, "title").unwrap_or_default().trim().to_string();
    check_title(&title, &mut errors);
    let state = mutation.state();
    let handle = match text(&input, "handle").map(lsf_core::util::handleize) {
        Some(handle) if handle_taken(state, &handle, None) => {
            errors.push(
                UserError::new(
                    &["handle"],
                    format!("Handle '{handle}' already in use. Please provide a new handle."),
                )
                .code("TAKEN"),
            );
            handle
        }
        Some(handle) if !handle.is_empty() => handle,
        _ => free_handle(state, &title),
    };
    let options = options_of(&input, &mut errors);
    let join = collections(state, &input, "collectionsToJoin", &mut errors);
    let metafields = objects(&input, "metafields");
    errors.extend(records::check_metafields(
        state,
        None,
        "PRODUCT",
        &metafields,
        &["metafields"],
        None,
    ));
    let media_inputs = media::media_arg(ctx);
    let (media_errors, _) = media::check_media(&media_inputs, &["media"]);
    errors.extend(media_errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"product": null}), &errors));
    }

    let state = mutation.state_mut();
    let (product, mut record) = new_record(state, &title, &handle);
    let gift_card = input.get("giftCard").and_then(Value::as_bool);
    record.insert("isGiftCard".into(), json!(gift_card.unwrap_or(false)));
    apply_fields(&mut record, &input);
    if let Some(role) = text(&input, "combinedListingRole") {
        record.insert("combinedListingRole".into(), json!(role));
    }
    state.insert(record);
    let choices = create_options(state, &product, &options);
    for input in &media_inputs {
        media::create_media(state, &product, input);
    }
    let stock = variants::default_stock(state);
    let variant = variants::insert_variant(state, &product, &choices, &Map::new(), &stock);
    records::set_metafields(state, &product, "PRODUCT", &metafields);
    join_collections(state, &product, &join, &[]);
    product_changed(state, &product);

    state.emit("products/create", &product);
    emit_new_inventory(state, &variant);
    Ok(payload(json!({"product": product}), &[]))
}

/// A new product record, not inserted yet: no options, variants or media, unpublished, `ACTIVE`,
/// the shop's name as its vendor. Returns its GID with it.
pub fn new_record(state: &mut ShopState, title: &str, handle: &str) -> (String, Record) {
    let now = state.timestamp();
    let vendor = state
        .shop()
        .and_then(|shop| records::field(shop, "name"))
        .unwrap_or_default()
        .to_string();
    let origin = records::origin(state);
    let product = state.new_gid("Product");
    let record = value::object(json!({
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
    }));
    (product, record)
}

/// Creates the options of a new product, each value a `ProductOptionValue`: the first value of
/// each option is the one its first variant selects. Returns those choices.
pub fn create_options(
    state: &mut ShopState,
    product: &str,
    options: &[(String, Vec<String>)],
) -> Vec<Choice> {
    let now = state.timestamp();
    let mut option_ids = Vec::new();
    let mut choices = Vec::new();
    for (position, (name, values)) in options.iter().enumerate() {
        let option = variants::add_option(state, name, position + 1, &now);
        for (index, value) in values.iter().enumerate() {
            let id = variants::add_value(state, &option, value);
            if index == 0 {
                choices.push(Choice {
                    option: option.clone(),
                    name: value.clone(),
                    value: Some(id),
                });
            }
        }
        option_ids.push(option);
    }
    if let Some(record) = state.get_mut(product) {
        record.insert("options".into(), json!(option_ids));
    }
    choices
}

/// Emits `inventory_items/create` and `inventory_levels/connect` for a new variant.
pub fn emit_new_inventory(state: &mut ShopState, variant: &str) {
    for (topic, key) in new_inventory_events(state, variant) {
        state.emit(topic, &key);
    }
}

/// The events of a new variant's inventory, in order: `inventory_items/create`, then
/// `inventory_levels/connect` for each level.
pub fn new_inventory_events(state: &ShopState, variant: &str) -> Vec<(&'static str, String)> {
    let Some(item) = item_of(state, variant) else {
        return Vec::new();
    };
    let levels = state
        .get(&item)
        .map(|record| gids(record.get("inventoryLevels")))
        .unwrap_or_default();
    std::iter::once(("inventory_items/create", item))
        .chain(
            levels
                .into_iter()
                .map(|level| ("inventory_levels/connect", level)),
        )
        .collect()
}

/// The product a `productUpdate` names: `product.id`, else `identifier` (`id`, `handle`).
fn product_to_update(
    state: &ShopState,
    ctx: &FieldContext<'_>,
    input: &Map<String, Value>,
) -> Option<String> {
    let identifier = ctx.arg("identifier").and_then(Value::as_object);
    let id = text(input, "id").or_else(|| identifier.and_then(|identifier| text(identifier, "id")));
    if let Some(id) = id {
        return (state.type_of(id) == Some("Product")).then(|| id.to_string());
    }
    if let Some(custom) = identifier.and_then(|identifier| identifier.get("customId")) {
        return product_by_custom_id(state, custom);
    }
    let handle = identifier.and_then(|identifier| text(identifier, "handle"))?;
    records::find_by_handle(state, "Product", handle)
}

/// The product that owns the metafield of a `UniqueMetafieldValueInput` (`{namespace, key,
/// value}`; the namespace defaults to the app's), as `customId` finds it.
pub fn product_by_custom_id(state: &ShopState, custom: &Value) -> Option<String> {
    let namespace = crate::domains::content::admin_namespace(
        records::app_id(state),
        custom
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or("$app"),
    );
    let key = custom.get("key").and_then(Value::as_str)?;
    let wanted = custom.get("value").and_then(Value::as_str)?;
    state.keys("Product").find_map(|product| {
        let record = state.get(product)?;
        gids(record.get("metafields"))
            .iter()
            .filter_map(|metafield| state.get(metafield))
            .any(|metafield| {
                records::field(metafield, "namespace") == Some(namespace.as_str())
                    && records::field(metafield, "key") == Some(key)
                    && records::field(metafield, "value") == Some(wanted)
            })
            .then(|| product.to_string())
    })
}

/// `productUpdate`.
fn product_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = product_input(ctx);
    note_ignored(mutation, "productUpdate", &input);
    if input
        .get("combinedListingRole")
        .is_some_and(|role| !role.is_null())
    {
        mutation.note("productUpdate: `combinedListingRole` can only be set when a product is created: it was ignored");
    }
    let state = mutation.state();
    let Some(product) = product_to_update(state, ctx, &input) else {
        let error =
            UserError::new(&["id"], "Product does not exist").code("PRODUCT_DOES_NOT_EXIST");
        return Ok(payload(json!({"product": null}), &[error]));
    };
    let mut errors = Vec::new();
    if let Some(title) = text(&input, "title") {
        check_title(title, &mut errors);
    }
    let handle = text(&input, "handle").map(lsf_core::util::handleize);
    if let Some(handle) = &handle
        && (handle.is_empty() || handle_taken(state, handle, Some(&product)))
    {
        errors.push(
            UserError::new(
                &["handle"],
                format!("Handle '{handle}' already in use. Please provide a new handle."),
            )
            .code("TAKEN"),
        );
    }
    let join = collections(state, &input, "collectionsToJoin", &mut errors);
    let leave = collections(state, &input, "collectionsToLeave", &mut errors);
    let metafields = objects(&input, "metafields");
    errors.extend(records::check_metafields(
        state,
        Some(&product),
        "PRODUCT",
        &metafields,
        &["metafields"],
        None,
    ));
    let media_inputs = media::media_arg(ctx);
    let (media_errors, _) = media::check_media(&media_inputs, &["media"]);
    errors.extend(media_errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"product": null}), &errors));
    }

    let state = mutation.state_mut();
    let origin = records::origin(state);
    let Some(record) = state.get_mut(&product) else {
        return Ok(payload(json!({"product": null}), &[]));
    };
    apply_fields(record, &input);
    if let Some(handle) = &handle {
        record.insert("handle".into(), json!(handle));
        let url = format!("{origin}/products/{handle}");
        if record
            .get("onlineStoreUrl")
            .is_some_and(|url| !url.is_null())
        {
            record.insert("onlineStoreUrl".into(), json!(url));
        }
        record.insert("onlineStorePreviewUrl".into(), json!(url));
    }
    for input in &media_inputs {
        media::create_media(state, &product, input);
    }
    records::set_metafields(state, &product, "PRODUCT", &metafields);
    join_collections(state, &product, &join, &leave);
    state.touch(&product);
    product_changed(state, &product);
    state.emit("products/update", &product);
    Ok(payload(json!({"product": product}), &[]))
}

/// `productDelete`.
fn product_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx
        .arg("input")
        .and_then(|input| input.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if mutation.state().type_of(&id) != Some("Product") {
        let error =
            UserError::new(&["id"], "Product does not exist").code("PRODUCT_DOES_NOT_EXIST");
        return Ok(payload(
            json!({"deletedProductId": null, "productDeleteOperation": null}),
            &[error],
        ));
    }
    let state = mutation.state_mut();
    delete_product(state, &id);
    Ok(payload(
        json!({"deletedProductId": id, "productDeleteOperation": null}),
        &[],
    ))
}

/// `productDuplicate`.
fn product_duplicate(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"newProduct": null, "imageJob": null, "productDuplicateOperation": null});
    let state = mutation.state();
    let original = ctx.arg_str("productId").unwrap_or_default().to_string();
    if !records::is(state, &original, "Product") {
        let error = UserError::new(&["productId"], "Product does not exist");
        return Ok(payload(empty, &[error]));
    }
    let title = ctx
        .arg_str("newTitle")
        .unwrap_or_default()
        .trim()
        .to_string();
    let mut errors = Vec::new();
    check_title(&title, &mut errors);
    if !errors.is_empty() {
        for error in &mut errors {
            error.field = Some(vec!["newTitle".to_string()]);
        }
        return Ok(payload(empty, &errors));
    }
    let include_images = ctx.arg_bool("includeImages").unwrap_or(false);
    if ctx.arg_bool("includeTranslations") == Some(true) {
        mutation.note("productDuplicate: lsf does not simulate translations: `includeTranslations` was ignored");
    }
    let state = mutation.state_mut();
    let copy = duplicate(
        state,
        &original,
        &title,
        ctx.arg_str("newStatus"),
        include_images,
    );
    let image_job =
        include_images.then(|| crate::resolve::done_job(state, "product_duplicate_images"));
    if ctx.arg_bool("synchronous") == Some(false) {
        let operation = state.new_gid("ProductDuplicateOperation");
        state.insert(value::object(json!({
            "__typename": "ProductDuplicateOperation",
            "id": operation,
            "product": original,
            "newProduct": copy,
            "status": "COMPLETE",
            "userErrors": [],
        })));
        return Ok(payload(
            json!({"newProduct": null, "imageJob": image_job, "productDuplicateOperation": operation}),
            &[],
        ));
    }
    Ok(payload(
        json!({"newProduct": copy, "imageJob": image_job, "productDuplicateOperation": null}),
        &[],
    ))
}

/// Copies a product (see the module's documentation). Emits the events of the copy; returns its
/// GID.
fn duplicate(
    state: &mut ShopState,
    original: &str,
    title: &str,
    status: Option<&str>,
    include_images: bool,
) -> String {
    let Some(source) = state.get(original).cloned() else {
        return String::new();
    };
    let now = state.timestamp();
    let product = state.new_gid("Product");
    let handle = free_handle(state, title);
    let origin = records::origin(state);
    let mut record = source.clone();
    for (name, value) in [
        ("id", json!(product)),
        ("title", json!(title)),
        ("handle", json!(handle)),
        ("createdAt", json!(now)),
        ("updatedAt", json!(now)),
        ("publishedAt", Value::Null),
        ("onlineStoreUrl", Value::Null),
        (
            "onlineStorePreviewUrl",
            json!(format!("{origin}/products/{handle}")),
        ),
        ("options", json!([])),
        ("variants", json!([])),
        ("media", json!([])),
        ("images", json!([])),
        ("collections", json!([])),
        ("sellingPlanGroups", json!([])),
        ("metafields", json!([])),
    ] {
        record.insert(name.into(), value);
    }
    if let Some(status) = status {
        record.insert("status".into(), json!(status));
    }
    state.insert(record);
    // The options and their values, the choices of each variant mapped to the copies.
    let mut value_map: Vec<(String, String, String)> = Vec::new();
    let options: Vec<(String, Vec<String>)> = gids(source.get("options"))
        .iter()
        .filter_map(|option| {
            let record = state.get(option)?;
            let name = records::field(record, "name")?.to_string();
            let values = gids(record.get("optionValues"))
                .iter()
                .filter_map(|value| Some(records::field(state.get(value)?, "name")?.to_string()))
                .collect();
            Some((name, values))
        })
        .collect();
    create_options(state, &product, &options);
    let new_options = variants::options_of(state, &product);
    for option in &new_options {
        for (id, name) in &option.values {
            value_map.push((option.name.clone(), name.clone(), id.clone()));
        }
    }
    if include_images {
        for medium in gids(source.get("media")) {
            let Some(record) = state.get(&medium) else {
                continue;
            };
            if records::field(record, "__typename") != Some("MediaImage") {
                continue;
            }
            let url = record
                .get("image")
                .and_then(|image| image.get("url"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let alt = record.get("alt").cloned().unwrap_or(Value::Null);
            let input = value::object(
                json!({"originalSource": url, "alt": alt, "mediaContentType": "IMAGE"}),
            );
            media::create_media(state, &product, &input);
        }
    }
    let mut created = Vec::new();
    for variant in gids(source.get("variants")) {
        let Some(record) = state.get(&variant).cloned() else {
            continue;
        };
        let choices: Vec<Choice> = record
            .get("selectedOptions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|selected| {
                let option = selected.get("name")?.as_str()?;
                let name = selected.get("value")?.as_str()?;
                let option_id = new_options
                    .iter()
                    .find(|known| known.name == option)?
                    .id
                    .clone();
                let value = value_map
                    .iter()
                    .find(|(known_option, known_name, _)| {
                        known_option == option && known_name == name
                    })
                    .map(|(_, _, id)| id.clone());
                Some(Choice {
                    option: option_id,
                    name: name.to_string(),
                    value,
                })
            })
            .collect();
        let item = item_of(state, &variant).and_then(|item| state.get(&item).cloned());
        let mut input = Map::new();
        for name in [
            "price",
            "compareAtPrice",
            "inventoryPolicy",
            "taxable",
            "showUnitPrice",
        ] {
            if let Some(value) = record.get(name).filter(|value| !value.is_null()) {
                input.insert(name.into(), value.clone());
            }
        }
        if let Some(barcodes) = record.get("barcodes").filter(|value| value.is_array()) {
            input.insert("barcodes".into(), barcodes.clone());
        }
        if let Some(item) = &item {
            let mut item_input = Map::new();
            for name in [
                "sku",
                "tracked",
                "requiresShipping",
                "countryCodeOfOrigin",
                "provinceCodeOfOrigin",
                "harmonizedSystemCode",
            ] {
                if let Some(value) = item.get(name).filter(|value| !value.is_null()) {
                    item_input.insert(name.into(), value.clone());
                }
            }
            if let Some(weight) = item.get("measurement").and_then(|m| m.get("weight")) {
                item_input.insert("measurement".into(), json!({"weight": weight}));
            }
            input.insert("inventoryItem".into(), Value::Object(item_input));
        }
        // The copy is stocked where the original is, with nothing.
        let stock: variants::Stock = item
            .map(|item| gids(item.get("inventoryLevels")))
            .unwrap_or_default()
            .iter()
            .filter_map(|level| records::field(state.get(level)?, "location").map(str::to_string))
            .map(|location| (location, 0))
            .collect();
        let copy = variants::insert_variant(state, &product, &choices, &input, &stock);
        let unit = record.get("unitPriceMeasurement").cloned();
        if let Some(copy_record) = state.get_mut(&copy)
            && let Some(unit) = unit
        {
            copy_record.insert("unitPriceMeasurement".into(), unit);
        }
        created.push(copy);
    }
    // Metafields and collections.
    let metafields: Vec<Map<String, Value>> = gids(source.get("metafields"))
        .iter()
        .filter_map(|metafield| state.get(metafield))
        .map(|metafield| {
            value::object(json!({
                "namespace": metafield.get("namespace"),
                "key": metafield.get("key"),
                "type": metafield.get("type"),
                "value": metafield.get("value"),
            }))
        })
        .collect();
    let metafields: Vec<&Map<String, Value>> = metafields.iter().collect();
    records::set_metafields(state, &product, "PRODUCT", &metafields);
    let collections = gids(source.get("collections"));
    join_collections(state, &product, &collections, &[]);
    product_changed(state, &product);
    state.emit("products/create", &product);
    for variant in &created {
        emit_new_inventory(state, variant);
    }
    product
}

/// `productChangeStatus` (deprecated: `productUpdate` sets `status`).
fn product_change_status(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let product = ctx.arg_str("productId").unwrap_or_default().to_string();
    if !records::is(mutation.state(), &product, "Product") {
        // shopify.dev's example.
        let error =
            UserError::new(&["productId"], "Product does not exist").code("PRODUCT_NOT_FOUND");
        return Ok(payload(json!({"product": null}), &[error]));
    }
    let status = ctx.arg_str("status").unwrap_or("ACTIVE").to_string();
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&product) {
        record.insert("status".into(), json!(status));
    }
    state.touch(&product);
    product_changed(state, &product);
    state.emit("products/update", &product);
    Ok(payload(json!({"product": product}), &[]))
}

/// Removes a product and what is its alone (options, variants, inventory, media, metafields),
/// and takes it out of the collections, the publications and the selling plan groups. Emits
/// `inventory_items/delete` for each inventory item and `products/delete`.
pub fn delete_product(state: &mut ShopState, product: &str) {
    let Some(record) = state.get(product).cloned() else {
        return;
    };
    let mut owned: Vec<String> = Vec::new();
    for option in gids(record.get("options")) {
        if let Some(option_record) = state.get(&option) {
            owned.extend(gids(option_record.get("optionValues")));
        }
        owned.push(option);
    }
    for media in gids(record.get("media")) {
        owned.push(media);
    }
    let mut metafields = gids(record.get("metafields"));
    let variants = gids(record.get("variants"));
    // The bundles they were components of no longer list them, nor the price lists.
    super::bundles::removing_variants(state, &variants);
    crate::domains::markets::removing_variants(state, &variants);
    for variant in &variants {
        if let Some(variant_record) = state.get(variant).cloned() {
            metafields.extend(gids(variant_record.get("metafields")));
            if let Some(item) = variant_record.get("inventoryItem").and_then(Value::as_str) {
                let levels = state
                    .get(item)
                    .map(|item| gids(item.get("inventoryLevels")))
                    .unwrap_or_default();
                for level in levels {
                    let location = state
                        .get(&level)
                        .and_then(|level| level.get("location"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if let Some(Value::Array(location_levels)) = location
                        .and_then(|location| state.get_mut(&location))
                        .and_then(|record| record.get_mut("inventoryLevels"))
                    {
                        location_levels.retain(|item| item.as_str() != Some(level.as_str()));
                    }
                    state.remove(&level);
                }
                state.delete(item, "inventory_items/delete");
            }
        }
        state.remove(variant);
    }
    // Through the content family, so that their definitions no longer list them.
    for metafield in metafields {
        crate::domains::content::metafields::remove(state, &metafield);
    }
    for key in owned {
        state.remove(&key);
    }
    let referrers: Vec<String> = ["Collection", "Publication", "SellingPlanGroup"]
        .iter()
        .flat_map(|type_name| {
            state
                .keys(type_name)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    // The listings of the sales channels that have it.
    crate::domains::markets::removing(state, product);
    for referrer in referrers {
        if let Some(record) = state.get_mut(&referrer) {
            if let Some(Value::Array(products)) = record.get_mut("products") {
                products.retain(|item| item.as_str() != Some(product));
            }
            // A selling plan group lists the variants it has apart (as `remove_variant` keeps).
            if let Some(Value::Array(listed)) = record.get_mut("productVariants") {
                listed.retain(|item| {
                    !item
                        .as_str()
                        .is_some_and(|known| variants.iter().any(|variant| variant == known))
                });
            }
        }
    }
    state.delete(product, "products/delete");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tags_as_shopify() {
        assert_eq!(
            tags_of(Some(&json!(["a, b", "B", " c "]))),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(tags_of(None).is_empty());
    }
}
