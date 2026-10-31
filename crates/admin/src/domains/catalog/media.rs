//! The media of products: `productCreateMedia`, `productUpdateMedia`, `productDeleteMedia`,
//! `productReorderMedia`, `productVariantAppendMedia`, `productVariantDetachMedia`, and the
//! `media` input of `productCreate`, `productUpdate` and the variant mutations.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - Media given by URL (`originalSource`) is recorded without being downloaded: an image's
//!   `image.url` is the URL given, its size unknown. Shopify answers a new medium `UPLOADED`
//!   and processes it in the background; lsf has nothing to process, and answers it `READY` at
//!   once, so that a backend waiting for `READY` goes on. An `originalSource` that is not an
//!   `http(s)` URL is refused as Shopify refuses it: "Image URL is invalid" (the reference's
//!   example, `field` `["media", "1", "originalSource"]`), and the valid media of the same call
//!   are still created (the same example).
//! - `productCreateMedia`, `productUpdateMedia`, `productDeleteMedia` are deprecated in favor of
//!   `productUpdate(media:)`, `fileUpdate`, but still served: lsf simulates them.
//! - `productReorderMedia` is asynchronous on Shopify: it answers a `Job`. lsf moves the media at
//!   once and answers a job that is already `done`.
//! - Every change of a product's media is a `products/update` (the product payload lists its
//!   images and media).
//!
//! The id of a new image is the one lsf-core gives the image of a product at its position
//! (`stable_id("media", "<handle>/<position>")`), when no record has it, so that the store sync
//! writes it back with the id it has (see "What the projection reads, for the mutations" in
//! `crates/admin/docs/design.md`).

use serde_json::{Map, Value, json};

use super::records::{coded, field, is, text};
use super::{gids, refresh_product};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("productCreateMedia", product_create_media)
        .mutation("productUpdateMedia", product_update_media)
        .mutation("productDeleteMedia", product_delete_media)
        .mutation("productReorderMedia", product_reorder_media)
        .mutation("productVariantAppendMedia", variant_append_media)
        .mutation("productVariantDetachMedia", variant_detach_media);
}

/// The media types a product can have.
pub const MEDIA_TYPES: [&str; 4] = ["MediaImage", "Video", "ExternalVideo", "Model3d"];

/// Whether `key` is a medium (of any type).
pub fn is_media(state: &ShopState, key: &str) -> bool {
    state
        .type_of(key)
        .is_some_and(|type_name| MEDIA_TYPES.contains(&type_name))
}

/// Checks the `CreateMediaInput`s of a call: the errors of those that cannot be created, under
/// `path` (`["media"]`), and whether each can be.
pub fn check_media(inputs: &[&Map<String, Value>], path: &[&str]) -> (Vec<UserError>, Vec<bool>) {
    let mut errors = Vec::new();
    let mut valid = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let source = text(input, "originalSource").unwrap_or_default().trim();
        let ok = source.starts_with("https://") || source.starts_with("http://");
        if !ok {
            let position = index.to_string();
            let mut field_path: Vec<&str> = path.to_vec();
            field_path.extend([position.as_str(), "originalSource"]);
            let message = match text(input, "mediaContentType") {
                Some("VIDEO") => "Video URL is invalid",
                Some("EXTERNAL_VIDEO") => "External video URL is invalid",
                Some("MODEL_3D") => "Model3d URL is invalid",
                // shopify.dev's example of productCreateMedia.
                _ => "Image URL is invalid",
            };
            errors.push(coded(&field_path, message, "INVALID"));
        }
        valid.push(ok);
    }
    (errors, valid)
}

/// Creates a medium of a product from a `CreateMediaInput`, at the end of its media. Returns
/// its GID.
pub fn create_media(state: &mut ShopState, product: &str, input: &Map<String, Value>) -> String {
    let now = state.timestamp();
    let source = text(input, "originalSource")
        .unwrap_or_default()
        .trim()
        .to_string();
    let alt = text(input, "alt").unwrap_or_default().to_string();
    let handle = state
        .get(product)
        .and_then(|record| field(record, "handle"))
        .unwrap_or_default()
        .to_string();
    let position = state
        .get(product)
        .map_or(0, |record| gids(record.get("media")).len());
    let kind = text(input, "mediaContentType").unwrap_or("IMAGE");
    let type_name = match kind {
        "VIDEO" => "Video",
        "EXTERNAL_VIDEO" => "ExternalVideo",
        "MODEL_3D" => "Model3d",
        _ => "MediaImage",
    };
    // The id lsf-core gives the image of a product at this position, when it is free.
    let lsf_core_id = gid(
        type_name,
        lsf_core::util::stable_id("media", &format!("{handle}/{position}")),
    );
    let id = if type_name == "MediaImage" && !state.contains(&lsf_core_id) {
        lsf_core_id
    } else {
        state.new_gid(type_name)
    };
    let file_name = source
        .split(['?', '#'])
        .next()
        .and_then(|path| path.rsplit('/').next())
        .unwrap_or_default()
        .to_string();
    let mime = super::mime_type(&file_name);
    let image = json!({
        "id": gid(
            "ImageSource",
            lsf_core::util::stable_id("image", &format!("{handle}/{source}")),
        ),
        "url": source,
        "altText": value::text(&alt),
        "width": null,
        "height": null,
    });
    let mut record = value::object(json!({
        "__typename": type_name,
        "id": id,
        "alt": value::text(&alt),
        "status": "READY",
        "fileStatus": "READY",
        "mediaContentType": kind,
        "preview": {"image": if type_name == "MediaImage" { image.clone() } else { Value::Null }, "status": "READY"},
        "createdAt": now,
        "updatedAt": now,
        "fileErrors": [],
        "mediaErrors": [],
        "mediaWarnings": [],
    }));
    let fields = match type_name {
        "MediaImage" => json!({
            "image": image,
            "mimeType": mime,
            "originalSource": {"url": source},
        }),
        "Video" => json!({
            "filename": file_name,
            "duration": null,
            "sources": [{"url": source, "mimeType": mime, "format": mime.rsplit('/').next(), "width": null, "height": null}],
            "originalSource": {"url": source, "mimeType": mime, "format": mime.rsplit('/').next(), "width": null, "height": null},
        }),
        "Model3d" => json!({
            "filename": file_name,
            "sources": [{"url": source, "mimeType": mime, "format": mime.rsplit('/').next()}],
            "originalSource": {"url": source, "mimeType": mime, "format": mime.rsplit('/').next()},
        }),
        _ => {
            let (host, embed, origin) = external_video(&source);
            json!({"host": host, "embedUrl": embed, "originUrl": origin})
        }
    };
    record.extend(value::object(fields));
    state.insert(record);
    if let Some(Value::Array(media)) = state
        .get_mut(product)
        .and_then(|record| record.get_mut("media"))
    {
        media.push(json!(id));
    }
    refresh_images(state, product);
    id
}

/// The host, embed URL and origin URL of an external video's URL (YouTube or Vimeo).
fn external_video(url: &str) -> (&'static str, String, String) {
    let id_after = |marker: &str| {
        url.split_once(marker)
            .map(|(_, rest)| {
                rest.split(['&', '?', '#', '/'])
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .filter(|id| !id.is_empty())
    };
    if url.contains("vimeo.com") {
        let id = id_after("vimeo.com/video/")
            .or_else(|| id_after("vimeo.com/"))
            .unwrap_or_default();
        return (
            "VIMEO",
            format!("https://player.vimeo.com/video/{id}"),
            format!("https://vimeo.com/{id}"),
        );
    }
    let id = id_after("v=")
        .or_else(|| id_after("youtu.be/"))
        .or_else(|| id_after("/embed/"))
        .unwrap_or_default();
    (
        "YOUTUBE",
        format!("https://www.youtube.com/embed/{id}"),
        format!("https://www.youtube.com/watch?v={id}"),
    )
}

/// Keeps a product's deprecated `images` (its `MediaImage`s' images, in order) and derived media
/// fields right.
pub fn refresh_images(state: &mut ShopState, product: &str) {
    let media = state
        .get(product)
        .map(|record| gids(record.get("media")))
        .unwrap_or_default();
    let images: Vec<Value> = media
        .iter()
        .filter_map(|media| state.get(media))
        .filter(|media| field(media, "__typename") == Some("MediaImage"))
        .filter_map(|media| media.get("image").filter(|image| !image.is_null()).cloned())
        .collect();
    if let Some(record) = state.get_mut(product) {
        record.insert("images".into(), json!(images));
    }
    refresh_product(state, product);
}

/// Attaches media to a variant: its `media` and its `image` (the first image).
pub fn attach_to_variant(state: &mut ShopState, variant: &str, media: &[String]) {
    let images: Vec<Value> = media
        .iter()
        .filter_map(|media| state.get(media))
        .filter_map(|media| media.get("image").filter(|image| !image.is_null()).cloned())
        .collect();
    if let Some(record) = state.get_mut(variant) {
        let mut current = gids(record.get("media"));
        for medium in media {
            if !current.contains(medium) {
                current.push(medium.clone());
            }
        }
        record.insert("media".into(), json!(current));
        if record.get("image").is_none_or(Value::is_null)
            && let Some(image) = images.first()
        {
            record.insert("image".into(), image.clone());
        }
    }
}

/// Detaches media from a variant, and its `image` when it was one of theirs.
pub fn detach_from_variant(state: &mut ShopState, variant: &str, media: &[String]) {
    let images: Vec<Value> = media
        .iter()
        .filter_map(|media| state.get(media))
        .filter_map(|media| media.get("image").cloned())
        .collect();
    if let Some(record) = state.get_mut(variant) {
        let mut current = gids(record.get("media"));
        current.retain(|medium| !media.contains(medium));
        record.insert("media".into(), json!(current));
        if record
            .get("image")
            .is_some_and(|image| images.contains(image))
        {
            record.insert("image".into(), Value::Null);
        }
    }
}

/// Removes media from a product: from its variants and from the shop. Returns the GIDs of the
/// images of those that were `MediaImage`s, as `deletedProductImageIds` lists them.
pub fn remove_media(state: &mut ShopState, product: &str, media: &[String]) -> Vec<String> {
    let variants = state
        .get(product)
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default();
    for variant in &variants {
        detach_from_variant(state, variant, media);
    }
    let mut images = Vec::new();
    for medium in media {
        if let Some(record) = state.get(medium)
            && field(record, "__typename") == Some("MediaImage")
            && let Some(image) = record
                .get("image")
                .and_then(|image| image.get("id"))
                .and_then(Value::as_str)
        {
            images.push(image.replace("ImageSource", "ProductImage"));
        }
        state.remove(medium);
    }
    if let Some(Value::Array(list)) = state
        .get_mut(product)
        .and_then(|record| record.get_mut("media"))
    {
        list.retain(|known| {
            known
                .as_str()
                .is_none_or(|known| !media.iter().any(|m| m == known))
        });
    }
    refresh_images(state, product);
    images
}

/// The product of `productId`, or the error of a product that is not one.
fn product_arg(state: &ShopState, ctx: &FieldContext<'_>, name: &str) -> Result<String, UserError> {
    let id = ctx.arg_str(name).unwrap_or_default();
    if is(state, id, "Product") {
        Ok(id.to_string())
    } else {
        Err(coded(
            &[name],
            "Product does not exist",
            "PRODUCT_DOES_NOT_EXIST",
        ))
    }
}

/// A payload with both `mediaUserErrors` and the deprecated `userErrors` (the same errors,
/// without codes).
fn media_payload(fields: Value, errors: &[UserError]) -> Value {
    let mut payload = payload(fields, errors);
    if let Some(map) = payload.as_object_mut() {
        let coded = map.remove("userErrors").unwrap_or_else(|| json!([]));
        let plain: Vec<Value> = coded
            .as_array()
            .into_iter()
            .flatten()
            .map(|error| json!({"field": error.get("field"), "message": error.get("message")}))
            .collect();
        map.insert("mediaUserErrors".into(), coded);
        map.insert("userErrors".into(), json!(plain));
    }
    payload
}

/// `productCreateMedia`: creates the valid media, reports the others.
fn product_create_media(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let product = match product_arg(mutation.state(), ctx, "productId") {
        Ok(product) => product,
        Err(error) => {
            return Ok(media_payload(
                json!({"media": null, "product": null}),
                &[error],
            ));
        }
    };
    let inputs = super::records::arg_objects(ctx, "media");
    let (errors, valid) = check_media(&inputs, &["media"]);
    let state = mutation.state_mut();
    let mut created = Vec::new();
    for (input, ok) in inputs.iter().zip(valid) {
        if ok {
            created.push(create_media(state, &product, input));
        }
    }
    if !created.is_empty() {
        state.touch(&product);
        state.emit("products/update", &product);
    }
    // The valid media are created even when others are not (shopify.dev's example).
    mutation.keep_changes();
    Ok(media_payload(
        json!({"media": created, "product": product}),
        &errors,
    ))
}

/// `productUpdateMedia`: the alt text of media of the product.
fn product_update_media(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let product = match product_arg(state, ctx, "productId") {
        Ok(product) => product,
        Err(error) => {
            return Ok(media_payload(
                json!({"media": null, "product": null}),
                &[error],
            ));
        }
    };
    let own = state
        .get(&product)
        .map(|record| gids(record.get("media")))
        .unwrap_or_default();
    let inputs = super::records::arg_objects(ctx, "media");
    let mut errors = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let id = text(input, "id").unwrap_or_default();
        if !is_media(state, id) {
            errors.push(coded(
                &["media", &index.to_string(), "id"],
                &format!("Media id {id} does not exist"),
                "MEDIA_DOES_NOT_EXIST",
            ));
        } else if !own.iter().any(|known| known == id) {
            errors.push(coded(
                &["media", &index.to_string(), "id"],
                &format!("Media id {id} does not exist on the given product"),
                "MEDIA_DOES_NOT_EXIST_ON_PRODUCT",
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(media_payload(
            json!({"media": null, "product": product}),
            &errors,
        ));
    }
    if inputs
        .iter()
        .any(|input| input.contains_key("previewImageSource"))
    {
        mutation.note(
            "productUpdateMedia: lsf does not simulate `previewImageSource` yet: it was ignored",
        );
    }
    let state = mutation.state_mut();
    let mut updated = Vec::new();
    for input in &inputs {
        let id = text(input, "id").unwrap_or_default().to_string();
        if let Some(alt) = input.get("alt")
            && let Some(record) = state.get_mut(&id)
        {
            let alt = alt.as_str().unwrap_or_default();
            record.insert("alt".into(), value::text(alt));
            for name in ["image", "preview"] {
                if let Some(image) = record.get_mut(name).and_then(Value::as_object_mut) {
                    let image = match image.get_mut("image").and_then(Value::as_object_mut) {
                        Some(inner) => inner,
                        None => image,
                    };
                    if image.contains_key("url") {
                        image.insert("altText".into(), value::text(alt));
                    }
                }
            }
            state.touch(&id);
        }
        updated.push(id);
    }
    refresh_images(state, &product);
    state.touch(&product);
    state.emit("products/update", &product);
    Ok(media_payload(
        json!({"media": updated, "product": product}),
        &[],
    ))
}

/// `productDeleteMedia`.
fn product_delete_media(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let nothing = json!({"deletedMediaIds": null, "deletedProductImageIds": null});
    let product = match product_arg(state, ctx, "productId") {
        Ok(product) => product,
        Err(error) => {
            let mut fields = nothing.clone();
            fields["product"] = Value::Null;
            return Ok(media_payload(fields, &[error]));
        }
    };
    let own = state
        .get(&product)
        .map(|record| gids(record.get("media")))
        .unwrap_or_default();
    let ids = gids(ctx.arg("mediaIds"));
    let mut errors = Vec::new();
    for id in &ids {
        if !own.contains(id) {
            // shopify.dev's example: `field` `["mediaIds"]`.
            errors.push(coded(
                &["mediaIds"],
                &format!("Media id {id} does not exist"),
                "MEDIA_DOES_NOT_EXIST",
            ));
        }
    }
    if !errors.is_empty() {
        let mut fields = nothing;
        fields["product"] = json!(product);
        return Ok(media_payload(fields, &errors));
    }
    let state = mutation.state_mut();
    let images = remove_media(state, &product, &ids);
    state.touch(&product);
    state.emit("products/update", &product);
    Ok(media_payload(
        json!({"deletedMediaIds": ids, "deletedProductImageIds": images, "product": product}),
        &[],
    ))
}

/// A `Job` that is already done: what lsf answers for the work Shopify does in the background.
pub fn done_job(state: &mut ShopState, kind: &str) -> String {
    let id = format!(
        "gid://shopify/Job/{}",
        value::uuid(&format!(
            "job/{}/{kind}/{}",
            state.domain(),
            state.count("Job")
        ))
    );
    let mut record = Record::new();
    record.insert("__typename".into(), json!("Job"));
    record.insert("id".into(), json!(id));
    record.insert("done".into(), json!(true));
    state.insert_as(id.clone(), record);
    id
}

/// `productReorderMedia`.
fn product_reorder_media(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let product = match product_arg(state, ctx, "id") {
        Ok(product) => product,
        Err(error) => return Ok(media_payload(json!({"job": null}), &[error])),
    };
    let mut media = state
        .get(&product)
        .map(|record| gids(record.get("media")))
        .unwrap_or_default();
    let moves = super::records::arg_objects(ctx, "moves");
    let mut errors = Vec::new();
    for (index, change) in moves.iter().enumerate() {
        let id = text(change, "id").unwrap_or_default();
        if !media.iter().any(|known| known == id) {
            errors.push(coded(
                &["moves", &index.to_string(), "id"],
                "Media does not exist on the given product.",
                "MEDIA_DOES_NOT_EXIST_ON_PRODUCT",
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(media_payload(json!({"job": null}), &errors));
    }
    // Each move in turn: the medium leaves its place and takes `newPosition` (from 0), the last
    // place when it is past the end.
    for change in &moves {
        let id = text(change, "id").unwrap_or_default().to_string();
        let position = change
            .get("newPosition")
            .and_then(|position| {
                position
                    .as_u64()
                    .or_else(|| position.as_str().and_then(|text| text.parse().ok()))
            })
            .unwrap_or(0);
        media.retain(|known| *known != id);
        let at = usize::try_from(position)
            .unwrap_or(usize::MAX)
            .min(media.len());
        media.insert(at, id);
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&product) {
        record.insert("media".into(), json!(media));
    }
    refresh_images(state, &product);
    state.touch(&product);
    state.emit("products/update", &product);
    let job = done_job(state, "product_reorder_media");
    Ok(media_payload(json!({"job": job}), &[]))
}

/// The pairs of `variantMedia`, checked: the variant must be the product's, and each medium
/// one of its media.
fn variant_media_pairs(
    state: &ShopState,
    product: &str,
    ctx: &FieldContext<'_>,
    errors: &mut Vec<UserError>,
) -> Vec<(String, Vec<String>)> {
    let record = state.get(product);
    let variants = record
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default();
    let media = record
        .map(|record| gids(record.get("media")))
        .unwrap_or_default();
    let mut pairs = Vec::new();
    for (index, pair) in super::records::arg_objects(ctx, "variantMedia")
        .iter()
        .enumerate()
    {
        let position = index.to_string();
        let variant = text(pair, "variantId").unwrap_or_default().to_string();
        if !variants.contains(&variant) {
            errors.push(coded(
                &["variantMedia", &position, "variantId"],
                "Variant does not exist on the given product.",
                "PRODUCT_VARIANT_DOES_NOT_EXIST_ON_PRODUCT",
            ));
        }
        if pairs.iter().any(|(known, _)| *known == variant) {
            errors.push(coded(
                &["variantMedia", &position, "variantId"],
                "Variant specified in more than one pair.",
                "PRODUCT_VARIANT_SPECIFIED_MULTIPLE_TIMES",
            ));
        }
        let ids = gids(pair.get("mediaIds"));
        for id in &ids {
            if !media.contains(id) {
                errors.push(coded(
                    &["variantMedia", &position, "mediaIds"],
                    "Media does not exist on the given product.",
                    "MEDIA_DOES_NOT_EXIST_ON_PRODUCT",
                ));
            }
        }
        pairs.push((variant, ids));
    }
    pairs
}

/// `productVariantAppendMedia`.
fn variant_append_media(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let product = match product_arg(state, ctx, "productId") {
        Ok(product) => product,
        Err(error) => {
            return Ok(payload(
                json!({"product": null, "productVariants": null}),
                &[error],
            ));
        }
    };
    let mut errors = Vec::new();
    let pairs = variant_media_pairs(state, &product, ctx, &mut errors);
    for (index, (variant, ids)) in pairs.iter().enumerate() {
        if ids.len() > 1 {
            errors.push(coded(
                &["variantMedia", &index.to_string(), "mediaIds"],
                "Only one mediaId is allowed per variant-media input pair.",
                "TOO_MANY_MEDIA_PER_INPUT_PAIR",
            ));
        }
        let has_media = state
            .get(variant)
            .is_some_and(|record| !gids(record.get("media")).is_empty());
        if has_media {
            errors.push(coded(
                &["variantMedia", &index.to_string(), "variantId"],
                "Product variant already has attached media.",
                "PRODUCT_VARIANT_ALREADY_HAS_MEDIA",
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(
            json!({"product": null, "productVariants": null}),
            &errors,
        ));
    }
    let state = mutation.state_mut();
    for (variant, ids) in &pairs {
        attach_to_variant(state, variant, ids);
        state.touch(variant);
    }
    state.touch(&product);
    state.emit("products/update", &product);
    let variants: Vec<&String> = pairs.iter().map(|(variant, _)| variant).collect();
    Ok(payload(
        json!({"product": product, "productVariants": variants}),
        &[],
    ))
}

/// `productVariantDetachMedia`.
fn variant_detach_media(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let product = match product_arg(state, ctx, "productId") {
        Ok(product) => product,
        Err(error) => {
            return Ok(payload(
                json!({"product": null, "productVariants": null}),
                &[error],
            ));
        }
    };
    let mut errors = Vec::new();
    let pairs = variant_media_pairs(state, &product, ctx, &mut errors);
    for (index, (variant, ids)) in pairs.iter().enumerate() {
        let attached = state
            .get(variant)
            .map(|record| gids(record.get("media")))
            .unwrap_or_default();
        if ids.iter().any(|id| !attached.contains(id)) {
            errors.push(coded(
                &["variantMedia", &index.to_string(), "mediaIds"],
                "The specified media is not attached to the specified variant.",
                "MEDIA_IS_NOT_ATTACHED_TO_VARIANT",
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(
            json!({"product": null, "productVariants": null}),
            &errors,
        ));
    }
    let state = mutation.state_mut();
    for (variant, ids) in &pairs {
        detach_from_variant(state, variant, ids);
        state.touch(variant);
    }
    state.touch(&product);
    state.emit("products/update", &product);
    let variants: Vec<&String> = pairs.iter().map(|(variant, _)| variant).collect();
    Ok(payload(
        json!({"product": product, "productVariants": variants}),
        &[],
    ))
}

/// The media `mediaSrc` (URLs) or `mediaId` of a variant input name, among a product's media.
pub fn media_of_variant_input(
    state: &ShopState,
    product: &str,
    input: &Map<String, Value>,
) -> Vec<String> {
    let media = state
        .get(product)
        .map(|record| gids(record.get("media")))
        .unwrap_or_default();
    let mut found = Vec::new();
    if let Some(id) = text(input, "mediaId")
        && media.iter().any(|known| known == id)
    {
        found.push(id.to_string());
    }
    for source in gids(input.get("mediaSrc")) {
        if let Some(medium) = media.iter().find(|medium| {
            state.get(medium).is_some_and(|record| {
                record
                    .get("originalSource")
                    .and_then(|source| source.get("url"))
                    .and_then(Value::as_str)
                    == Some(source.as_str())
            })
        }) && !found.contains(medium)
        {
            found.push(medium.clone());
        }
    }
    found
}

/// The `media` argument of a mutation, as `CreateMediaInput`s.
pub fn media_arg<'c>(ctx: &FieldContext<'c>) -> Vec<&'c Map<String, Value>> {
    super::records::arg_objects(ctx, "media")
}
