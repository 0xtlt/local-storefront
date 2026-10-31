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
//!   Events: `products/create`, `inventory_items/create`, `inventory_levels/connect`.
//! - `productUpdate` changes the fields given (`tags` replaces the tags): `products/update`.
//! - `productDelete` removes the product, its options, variants, inventory and media, and the
//!   references of its collections: `products/delete`, `inventory_items/delete`.
//!
//! The messages are Shopify's (Rails validations: "Title can't be blank"); where shopify.dev does
//! not show them, they follow the same form. Inputs lsf does not simulate yet (`media`,
//! `metafields`, `category`, …) are said in `extensions.lsf.notes`, never silently dropped.

use serde_json::{Map, Value, json};

use super::{QUANTITY_NAMES, gids, refresh_product};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("productCreate", product_create)
        .mutation("productUpdate", product_update)
        .mutation("productDelete", product_delete);
}

/// The inputs of the product mutations that lsf does not simulate yet.
const NOT_SIMULATED: &[&str] = &[
    "category",
    "claimOwnership",
    "combinedListingRole",
    "metafields",
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

fn text<'i>(input: &'i Map<String, Value>, field: &str) -> Option<&'i str> {
    input.get(field).and_then(Value::as_str)
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
fn handle_taken(state: &ShopState, handle: &str, except: Option<&str>) -> bool {
    state.keys("Product").any(|key| {
        Some(key) != except
            && state
                .get(key)
                .and_then(|product| product.get("handle"))
                .and_then(Value::as_str)
                == Some(handle)
    })
}

/// A free handle made from `base`: itself, else with `-1`, `-2`, … appended.
fn free_handle(state: &ShopState, base: &str) -> String {
    let base = match lsf_core::util::handleize(base) {
        handle if handle.is_empty() => "product".to_string(),
        handle => handle,
    };
    if !handle_taken(state, &base, None) {
        return base;
    }
    (1..)
        .map(|n| format!("{base}-{n}"))
        .find(|handle| !handle_taken(state, handle, None))
        .unwrap_or(base)
}

/// The checks of a title.
fn check_title(title: &str, errors: &mut Vec<UserError>) {
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
fn collections(
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
fn note_ignored(
    mutation: &mut Mutation<'_>,
    name: &str,
    input: &Map<String, Value>,
    ctx: &FieldContext<'_>,
) {
    for field in NOT_SIMULATED {
        if input.get(*field).is_some_and(|value| !value.is_null()) {
            mutation.note(format!(
                "{name}: lsf does not simulate `{field}` yet: it was ignored"
            ));
        }
    }
    if ctx.arg("media").is_some() {
        mutation.note(format!(
            "{name}: lsf does not simulate `media` yet: it was ignored"
        ));
    }
}

/// Sets the plain fields of a product from an input: those given, and only those.
fn apply_fields(record: &mut Record, input: &Map<String, Value>) {
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

/// Adds the product to the collections (and the collections to the product), or takes it out.
fn join_collections(state: &mut ShopState, product: &str, join: &[String], leave: &[String]) {
    for collection in join.iter().chain(leave) {
        let joining = join.contains(collection);
        if let Some(Value::Array(products)) = state
            .get_mut(collection)
            .and_then(|record| record.get_mut("products"))
        {
            products.retain(|item| item.as_str() != Some(product));
            if joining {
                products.push(json!(product));
            }
        }
        if let Some(Value::Array(collections)) = state
            .get_mut(product)
            .and_then(|record| record.get_mut("collections"))
        {
            collections.retain(|item| item.as_str() != Some(collection.as_str()));
            if joining {
                collections.push(json!(collection));
            }
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
    note_ignored(mutation, "productCreate", &input, ctx);
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
    if !errors.is_empty() {
        return Ok(payload(json!({"product": null}), &errors));
    }

    let state = mutation.state_mut();
    let now = state.timestamp();
    let shop = state.shop().cloned().unwrap_or_default();
    let shop_text = |field: &str| {
        shop.get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let origin = shop_text("url");
    let product = state.new_gid("Product");

    let mut option_ids = Vec::new();
    let mut selected = Vec::new();
    for (position, (name, values)) in options.iter().enumerate() {
        let option = state.new_gid("ProductOption");
        let mut value_ids = Vec::new();
        for (index, value) in values.iter().enumerate() {
            let id = state.new_gid("ProductOptionValue");
            state.insert(value::object(json!({
                "__typename": "ProductOptionValue",
                "id": id,
                "name": value,
                "hasVariants": index == 0,
                "swatch": null,
            })));
            if index == 0 {
                selected.push(json!({"name": name, "value": value, "optionValue": id}));
            }
            value_ids.push(id);
        }
        state.insert(value::object(json!({
            "__typename": "ProductOption",
            "id": option,
            "name": name,
            "position": position + 1,
            "values": values,
            "optionValues": value_ids,
            "createdAt": now,
        })));
        option_ids.push(option);
    }
    let variant_title = options
        .iter()
        .filter_map(|(_, values)| values.first().cloned())
        .collect::<Vec<_>>()
        .join(" / ");

    let variant = state.new_gid("ProductVariant");
    let item = state.new_gid("InventoryItem");
    let location = state.keys("Location").next().map(str::to_string);
    let level = location.as_ref().map(|location| {
        let level = state.new_gid("InventoryLevel");
        let quantities: Vec<Value> = QUANTITY_NAMES
            .iter()
            .map(|name| json!({"name": name, "quantity": 0, "updatedAt": now}))
            .collect();
        state.insert(value::object(json!({
            "__typename": "InventoryLevel",
            "id": level,
            "item": item,
            "location": location,
            "quantities": quantities,
            "isActive": true,
            "canDeactivate": true,
            "createdAt": now,
            "updatedAt": now,
        })));
        if let Some(Value::Array(levels)) = state
            .get_mut(location)
            .and_then(|record| record.get_mut("inventoryLevels"))
        {
            levels.push(json!(level));
        }
        level
    });
    state.insert(value::object(json!({
        "__typename": "InventoryItem",
        "id": item,
        "sku": null,
        "tracked": false,
        "requiresShipping": true,
        "measurement": {"weight": {"unit": match shop_text("weightUnit").as_str() {
            "" => "KILOGRAMS".to_string(),
            unit => unit.to_string(),
        }, "value": 0.0}},
        "inventoryLevels": level.iter().collect::<Vec<_>>(),
        "variant": variant,
        "variants": [variant],
        "createdAt": now,
        "updatedAt": now,
    })));
    state.insert(value::object(json!({
        "__typename": "ProductVariant",
        "id": variant,
        "title": variant_title,
        "displayName": format!("{title} - {variant_title}"),
        "sku": null,
        "barcode": null,
        "barcodes": [],
        "price": "0.00",
        "compareAtPrice": null,
        "position": 1,
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

    let mut record = value::object(json!({
        "__typename": "Product",
        "id": product,
        "title": title,
        "handle": handle,
        "descriptionHtml": "",
        "bodyHtml": "",
        "description": "",
        "vendor": shop_text("name"),
        "productType": "",
        "tags": [],
        "status": "ACTIVE",
        "createdAt": now,
        "updatedAt": now,
        "publishedAt": null,
        "templateSuffix": null,
        "giftCardTemplateSuffix": null,
        "isGiftCard": input.get("giftCard").and_then(Value::as_bool).unwrap_or(false),
        "requiresSellingPlan": false,
        "hasVariantsThatRequiresComponents": false,
        "options": option_ids,
        "variants": [variant],
        "media": [],
        "images": [],
        "seo": {"title": null, "description": null},
        "onlineStoreUrl": null,
        "onlineStorePreviewUrl": format!("{origin}/products/{handle}"),
        "collections": [],
        "sellingPlanGroups": [],
        "metafields": [],
    }));
    apply_fields(&mut record, &input);
    state.insert(record);
    join_collections(state, &product, &join, &[]);
    refresh_product(state, &product);

    state.emit("products/create", &product);
    state.emit("inventory_items/create", &item);
    if let Some(level) = &level {
        state.emit("inventory_levels/connect", level);
    }
    Ok(payload(json!({"product": product}), &[]))
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
    let handle = identifier.and_then(|identifier| text(identifier, "handle"))?;
    state
        .keys("Product")
        .find(|key| {
            state
                .get(key)
                .and_then(|product| product.get("handle"))
                .and_then(Value::as_str)
                == Some(handle)
        })
        .map(str::to_string)
}

/// `productUpdate`.
fn product_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = product_input(ctx);
    note_ignored(mutation, "productUpdate", &input, ctx);
    if ctx
        .arg("identifier")
        .and_then(|identifier| identifier.get("customId"))
        .is_some()
    {
        mutation.note("productUpdate: lsf does not find a product by `customId` yet");
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
    if !errors.is_empty() {
        return Ok(payload(json!({"product": null}), &errors));
    }

    let state = mutation.state_mut();
    let origin = state
        .shop()
        .and_then(|shop| shop.get("url"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
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
    let title = record
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let variants = gids(record.get("variants"));
    if input.contains_key("title") {
        for variant in &variants {
            if let Some(variant) = state.get_mut(variant) {
                let variant_title = variant
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let display = format!("{title} - {variant_title}");
                variant.insert("displayName".into(), json!(display));
            }
        }
    }
    join_collections(state, &product, &join, &leave);
    state.touch(&product);
    refresh_product(state, &product);
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
    owned.extend(gids(record.get("metafields")));
    for variant in gids(record.get("variants")) {
        if let Some(variant_record) = state.get(&variant).cloned() {
            owned.extend(gids(variant_record.get("metafields")));
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
        state.remove(&variant);
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
    for referrer in referrers {
        if let Some(Value::Array(products)) = state
            .get_mut(&referrer)
            .and_then(|record| record.get_mut("products"))
        {
            products.retain(|item| item.as_str() != Some(product));
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
