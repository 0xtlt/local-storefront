//! Files: `fileCreate`, `fileUpdate`, `fileDelete`, and the search and sort of `files`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `fileCreate` takes up to 250 files, each from an external URL ("for images, generic files, or
//!   external videos") or a staged upload URL ("Videos and 3D models require" one). Its type is
//!   `contentType`, else the source's extension (`.jpg` an image, `.mp4` a video, `.glb` a 3D
//!   model, YouTube and Vimeo an external video, anything else a generic file).
//! - The filename is `filename`, else the last segment of the source; one already in use is
//!   handled by `duplicateResolutionMode`: `APPEND_UUID` (the default) appends a UUID,
//!   `RAISE_ERROR` refuses (`FILENAME_ALREADY_EXISTS`), `REPLACE` replaces that file, keeping its
//!   id (it needs a `filename`: `MISSING_FILENAME_FOR_DUPLICATE_MODE_REPLACE`). A `filename` must
//!   keep the source's extension (`MISMATCHED_FILENAME_AND_ORIGINAL_SOURCE`).
//! - "Files are processed asynchronously": lsf has nothing to process, so a file is `READY` at
//!   once (`fileStatus`, and `status` of a medium), which the polling of a backend sees on its
//!   first read. Its URL is the source's: where the bytes are (Shopify's would be on its CDN).
//!   Image sizes are not known (`width` and `height` are `null`; a video's are 1920×1080, the
//!   seed's choice).
//! - `fileUpdate` changes `alt`, `filename` (a video's, a 3D model's), the content
//!   (`originalSource`), and the products a file is the medium of (`referencesToAdd`,
//!   `referencesToRemove`; `products/update` for each).
//! - `fileDelete` removes the files, and from the media of the products that have them.
//!
//! Staged uploads (`stagedUploadsCreate` and lsf's upload endpoint) are the app family's. When a
//! source is the `resourceUrl` of a staged target that received a file, the bytes the shop keeps
//! for it ([`crate::domains::app::uploads::bytes`]) give the file its name (the last segment of
//! the key, the `filename` the target was made for), its MIME type (the one it was uploaded with)
//! and its size ([`staged_upload`]). Before anything was uploaded there, the URL is read like any
//! other.
//!
//! Messages: the descriptions of the codes where Shopify does not publish them.
//!
//! Events: Shopify has no webhook topic for files in 2026-10: their changes are the events
//! `files/create`, `files/update`, `files/delete`, which no subscription can name.

use serde_json::{Map, Value, json};

use crate::domains::app::uploads;
use crate::domains::catalog::{gids, mime_type};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::sort::SortValue;
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, gid_type, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("fileCreate", file_create)
        .mutation("fileUpdate", file_update)
        .mutation("fileDelete", file_delete)
        .default_text("File", |_r, record| {
            let mut texts = vec![filename_of(record)];
            texts.extend(
                record
                    .get("alt")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            );
            texts
        })
        .sort_key("File", "FILENAME", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .map(|record| json!(filename_of(record)))
                    .as_ref(),
            )
        })
        .sort_key("File", "ORIGINAL_UPLOAD_SIZE", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(file_size)
                    .map(|size| json!(size))
                    .as_ref(),
            )
        });
}

/// The most files one `fileCreate` creates.
const MAX_FILES: usize = 250;

/// What the shop knows of a file uploaded to a staged target: its name, its MIME type, its size
/// in bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedUpload {
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

/// The upload behind a staged target's `resourceUrl` (or its key), when a file was uploaded
/// there: the name is the last segment of the key (`tmp/<shop>/files/<uuid>/manual.pdf`), the
/// target's `filename`; the MIME type is the one the upload was sent with, without its
/// parameters (`text/plain; charset=utf-8` is `text/plain`).
pub fn staged_upload(state: &ShopState, url: &str) -> Option<StagedUpload> {
    let blob = uploads::bytes(state, url)?;
    let key = uploads::key_of(url)?;
    let mime_type = blob
        .content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    Some(StagedUpload {
        filename: last_segment(&key),
        mime_type: if mime_type.is_empty() {
            "application/octet-stream".to_string()
        } else {
            mime_type
        },
        size: blob.bytes.len() as u64,
    })
}

/// The URL of a file's bytes.
fn url_of(record: &Record) -> String {
    let candidates = [
        record.get("url"),
        record
            .get("originalSource")
            .and_then(|source| source.get("url")),
        record.get("image").and_then(|image| image.get("url")),
        record.get("originUrl"),
    ];
    candidates
        .into_iter()
        .flatten()
        .find_map(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The last segment of a URL's path, without its query.
fn last_segment(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// A file's name: its `filename`, else the last segment of its URL.
pub fn filename_of(record: &Record) -> String {
    match record.get("filename").and_then(Value::as_str) {
        Some(name) => name.to_string(),
        None => last_segment(&url_of(record)),
    }
}

/// A file's size in bytes, when lsf knows it.
fn file_size(record: &Record) -> Option<i64> {
    record
        .get("originalFileSize")
        .or_else(|| record.get("originalSource")?.get("fileSize"))
        .or_else(|| record.get("originalSource")?.get("filesize"))
        .and_then(Value::as_i64)
}

/// The extension of a name, lowercase.
fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default()
}

/// The `FileContentType` of a source without one: by its host and its extension.
fn content_type_of(url: &str, name: &str) -> &'static str {
    let host = url
        .split("//")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ["youtube.com", "youtu.be", "vimeo.com"]
        .iter()
        .any(|known| host == *known || host.ends_with(&format!(".{known}")))
    {
        return "EXTERNAL_VIDEO";
    }
    match extension(name).as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "svg" | "bmp" | "heic" | "tif"
        | "tiff" => "IMAGE",
        "mp4" | "m4v" | "mov" | "webm" => "VIDEO",
        "glb" | "usdz" => "MODEL_3D",
        _ => "FILE",
    }
}

/// The record type of a `FileContentType`.
fn record_type(content_type: &str) -> &'static str {
    match content_type {
        "IMAGE" => "MediaImage",
        "VIDEO" => "Video",
        "EXTERNAL_VIDEO" => "ExternalVideo",
        "MODEL_3D" => "Model3d",
        _ => "GenericFile",
    }
}

/// The files of the shop, of every type of `File`.
fn files(state: &ShopState) -> Vec<String> {
    [
        "MediaImage",
        "Video",
        "ExternalVideo",
        "Model3d",
        "GenericFile",
    ]
    .iter()
    .flat_map(|type_name| {
        state
            .keys(type_name)
            .map(str::to_string)
            .collect::<Vec<_>>()
    })
    .collect()
}

/// The file with a filename.
fn file_named(state: &ShopState, name: &str) -> Option<String> {
    files(state).into_iter().find(|id| {
        state
            .get(id)
            .is_some_and(|record| filename_of(record) == name)
    })
}

/// The fields of a file of `type_name` from a source: what both a new file and a replaced one
/// take.
fn content(
    type_name: &str,
    url: &str,
    name: &str,
    alt: Option<&str>,
    size: Option<u64>,
    mime: &str,
) -> Map<String, Value> {
    let format = extension(name);
    let fields = match type_name {
        "MediaImage" => {
            let image = json!({"url": url, "altText": alt, "width": null, "height": null});
            json!({
                "mediaContentType": "IMAGE",
                "mimeType": mime,
                "image": image,
                "preview": {"image": image, "status": "READY"},
                "originalSource": {"url": url, "fileSize": size},
                "status": "READY",
                "mediaErrors": [],
                "mediaWarnings": [],
            })
        }
        "Video" => {
            let source = json!({
                "url": url, "mimeType": mime, "format": format, "width": 1920, "height": 1080,
                "fileSize": size,
            });
            json!({
                "mediaContentType": "VIDEO",
                "filename": name,
                "sources": [source],
                "originalSource": source,
                "preview": {"image": null, "status": "READY"},
                "status": "READY",
                "mediaErrors": [],
                "mediaWarnings": [],
            })
        }
        "Model3d" => {
            let source = json!({"url": url, "mimeType": mime, "format": format, "filesize": size.unwrap_or(0)});
            json!({
                "mediaContentType": "MODEL_3D",
                "filename": name,
                "sources": [source],
                "originalSource": source,
                "preview": {"image": null, "status": "READY"},
                "status": "READY",
                "mediaErrors": [],
                "mediaWarnings": [],
            })
        }
        "ExternalVideo" => {
            let host = if url.contains("vimeo") {
                "VIMEO"
            } else {
                "YOUTUBE"
            };
            json!({
                "mediaContentType": "EXTERNAL_VIDEO",
                "host": host,
                "originUrl": url,
                "embedUrl": url,
                "embeddedUrl": url,
                "preview": {"image": null, "status": "READY"},
                "status": "READY",
                "mediaErrors": [],
                "mediaWarnings": [],
            })
        }
        _ => json!({
            "url": url,
            "mimeType": mime,
            "originalFileSize": size,
        }),
    };
    let mut fields = value::object(fields);
    fields.insert("fileStatus".into(), json!("READY"));
    fields.insert("fileErrors".into(), json!([]));
    fields
}

/// A source URL lsf takes: `http` or `https`, with a host.
fn valid_source(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
        && rest.split('/').next().is_some_and(|host| !host.is_empty())
}

/// A file `fileCreate` checked, ready to write.
struct Planned {
    type_name: &'static str,
    url: String,
    name: String,
    alt: Option<String>,
    size: Option<u64>,
    mime: String,
    replaces: Option<String>,
}

fn file_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let inputs: Vec<Map<String, Value>> = ctx
        .arg("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|input| input.as_object().cloned())
        .collect();
    if inputs.len() > MAX_FILES {
        let error = UserError::new(
            &["files"],
            format!(
                "The maximum number of files that can be created in a single batch is {MAX_FILES}."
            ),
        )
        .code("TOO_MANY_ARGUMENTS");
        return Ok(payload(json!({"files": null}), &[error]));
    }
    let state = mutation.state();
    let mut errors = Vec::new();
    let mut planned = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let index = index.to_string();
        let get = |name: &str| input.get(name).and_then(Value::as_str);
        let url = get("originalSource").unwrap_or_default().trim().to_string();
        if !valid_source(&url) {
            errors.push(
                UserError::new(
                    &["files", &index, "originalSource"],
                    "Image URL is invalid.",
                )
                .code("INVALID_IMAGE_SOURCE_URL"),
            );
            continue;
        }
        let staged = staged_upload(state, &url);
        let source_name = staged
            .as_ref()
            .map_or_else(|| last_segment(&url), |upload| upload.filename.clone());
        let mode = get("duplicateResolutionMode").unwrap_or("APPEND_UUID");
        let given_name = get("filename")
            .map(str::trim)
            .filter(|name| !name.is_empty());
        if let Some(name) = given_name
            && !source_name.is_empty()
            && extension(name) != extension(&source_name)
        {
            errors.push(
                UserError::new(
                    &["files", &index, "filename"],
                    "The filename extension does not match the original source's.",
                )
                .code("MISMATCHED_FILENAME_AND_ORIGINAL_SOURCE"),
            );
            continue;
        }
        if mode == "REPLACE" && given_name.is_none() {
            errors.push(
                UserError::new(
                    &["files", &index, "filename"],
                    "A filename is required for the duplicate resolution mode REPLACE.",
                )
                .code("MISSING_FILENAME_FOR_DUPLICATE_MODE_REPLACE"),
            );
            continue;
        }
        let content_type = get("contentType")
            .map(|kind| match kind {
                "EXTERNAL_VIDEO" => "EXTERNAL_VIDEO",
                "IMAGE" => "IMAGE",
                "VIDEO" => "VIDEO",
                "MODEL_3D" => "MODEL_3D",
                _ => "FILE",
            })
            .unwrap_or_else(|| content_type_of(&url, &source_name));
        let mut name = given_name.map_or(source_name, str::to_string);
        let taken = file_named(state, &name).or_else(|| names.contains(&name).then(String::new));
        let mut replaces = None;
        match (taken, mode) {
            (Some(_), "RAISE_ERROR") => {
                errors.push(
                    UserError::new(&["files", &index, "filename"], "Filename already exists.")
                        .code("FILENAME_ALREADY_EXISTS"),
                );
                continue;
            }
            (Some(existing), "REPLACE") if !existing.is_empty() => replaces = Some(existing),
            (Some(_), _) => {
                let uuid = value::uuid(&format!("file/{url}/{index}/{}", state.timestamp()));
                name = match name.rsplit_once('.') {
                    Some((stem, extension)) => format!("{stem}_{uuid}.{extension}"),
                    None => format!("{name}_{uuid}"),
                };
            }
            (None, _) => {}
        }
        names.push(name.clone());
        let mime = staged
            .as_ref()
            .map_or_else(|| mime_type(&name), |upload| upload.mime_type.clone());
        planned.push(Planned {
            type_name: record_type(content_type),
            url,
            name,
            alt: get("alt").map(str::to_string),
            size: staged.map(|upload| upload.size),
            mime,
            replaces,
        });
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"files": null}), &errors));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let mut ids = Vec::new();
    for plan in planned {
        let fields = content(
            plan.type_name,
            &plan.url,
            &plan.name,
            plan.alt.as_deref(),
            plan.size,
            &plan.mime,
        );
        if let Some(existing) = plan
            .replaces
            .filter(|existing| state.type_of(existing) == Some(plan.type_name))
        {
            if let Some(record) = state.get_mut(&existing) {
                record.extend(fields);
                if let Some(alt) = &plan.alt {
                    record.insert("alt".into(), json!(alt));
                }
            }
            state.touch(&existing);
            state.emit("files/update", &existing);
            ids.push(existing);
            continue;
        }
        let id = state.new_gid(plan.type_name);
        let mut record = value::object(json!({
            "__typename": plan.type_name,
            "id": id,
            "alt": plan.alt.clone().unwrap_or_default(),
            "createdAt": now,
            "updatedAt": now,
        }));
        record.extend(fields);
        if plan.type_name == "MediaImage" {
            record.insert("metafields".into(), json!([]));
        }
        state.insert(record);
        state.emit("files/create", &id);
        ids.push(id);
    }
    Ok(payload(json!({"files": ids}), &[]))
}

/// The products that have a file among their media.
fn products_with(state: &ShopState, file: &str) -> Vec<String> {
    state
        .keys("Product")
        .filter(|product| {
            state
                .get(product)
                .is_some_and(|record| gids(record.get("media")).iter().any(|media| media == file))
        })
        .map(str::to_string)
        .collect()
}

/// Adds a file to the media of a product, or takes it out.
fn set_reference(state: &mut ShopState, product: &str, file: &str, add: bool) -> bool {
    let Some(Value::Array(media)) = state
        .get_mut(product)
        .and_then(|record| record.get_mut("media"))
    else {
        return false;
    };
    let had = media.iter().any(|item| item.as_str() == Some(file));
    if add && !had {
        media.push(json!(file));
    } else if !add && had {
        media.retain(|item| item.as_str() != Some(file));
    } else {
        return false;
    }
    state.touch(product);
    crate::domains::catalog::refresh_product(state, product);
    state.emit("products/update", product);
    true
}

fn file_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let inputs: Vec<Map<String, Value>> = ctx
        .arg("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|input| input.as_object().cloned())
        .collect();
    let state = mutation.state();
    let mut errors = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let index = index.to_string();
        let id = input.get("id").and_then(Value::as_str).unwrap_or_default();
        let Some(record) = state.get(id).filter(|_| {
            matches!(
                gid_type(id),
                Some("MediaImage" | "Video" | "ExternalVideo" | "Model3d" | "GenericFile")
            )
        }) else {
            errors.push(
                UserError::new(
                    &["files", &index, "id"],
                    format!("File id {id} does not exist."),
                )
                .code("FILE_DOES_NOT_EXIST"),
            );
            continue;
        };
        if let Some(source) = input.get("originalSource").and_then(Value::as_str)
            && !valid_source(source)
        {
            errors.push(
                UserError::new(
                    &["files", &index, "originalSource"],
                    "Image URL is invalid.",
                )
                .code("INVALID_IMAGE_SOURCE_URL"),
            );
        }
        if let Some(name) = input.get("filename").and_then(Value::as_str) {
            if extension(name) != extension(&filename_of(record)) {
                errors.push(
                    UserError::new(
                        &["files", &index, "filename"],
                        "The filename extension can't be changed.",
                    )
                    .code("INVALID_FILENAME_EXTENSION"),
                );
            } else if file_named(state, name).is_some_and(|other| other != id) {
                errors.push(
                    UserError::new(&["files", &index, "filename"], "Filename already exists.")
                        .code("FILENAME_ALREADY_EXISTS"),
                );
            }
        }
        for field in ["referencesToAdd", "referencesToRemove"] {
            for reference in input
                .get(field)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if state.type_of(reference) != Some("Product") {
                    errors.push(
                        UserError::new(
                            &["files", &index, field],
                            "The target resource does not exist.",
                        )
                        .code("REFERENCE_TARGET_DOES_NOT_EXIST"),
                    );
                }
            }
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"files": null}), &errors));
    }
    let state = mutation.state_mut();
    let mut ids = Vec::new();
    for input in &inputs {
        let id = input
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Some(record) = state.get(&id).cloned() else {
            continue;
        };
        let type_name = state.type_of(&id).unwrap_or("GenericFile").to_string();
        let mut updated = record.clone();
        let name = input
            .get("filename")
            .and_then(Value::as_str)
            .map_or_else(|| filename_of(&record), str::to_string);
        if let Some(source) = input.get("originalSource").and_then(Value::as_str) {
            let staged = staged_upload(state, source);
            let mime = staged
                .as_ref()
                .map_or_else(|| mime_type(&name), |upload| upload.mime_type.clone());
            let alt = record
                .get("alt")
                .and_then(Value::as_str)
                .map(str::to_string);
            let type_static = record_type(match type_name.as_str() {
                "MediaImage" => "IMAGE",
                "Video" => "VIDEO",
                "Model3d" => "MODEL_3D",
                "ExternalVideo" => "EXTERNAL_VIDEO",
                _ => "FILE",
            });
            updated.extend(content(
                type_static,
                source,
                &name,
                alt.as_deref(),
                staged.map(|upload| upload.size),
                &mime,
            ));
        }
        if input.contains_key("filename") && updated.contains_key("filename") {
            updated.insert("filename".into(), json!(name));
        }
        if let Some(alt) = input.get("alt") {
            updated.insert("alt".into(), alt.clone());
            if let Some(Value::Object(image)) = updated.get_mut("image") {
                image.insert("altText".into(), alt.clone());
            }
        }
        if updated != record {
            if let Some(slot) = state.get_mut(&id) {
                *slot = updated;
            }
            state.touch(&id);
            state.emit("files/update", &id);
        }
        for (field, add) in [("referencesToAdd", true), ("referencesToRemove", false)] {
            for product in gids(input.get(field)) {
                set_reference(state, &product, &id, add);
            }
        }
        ids.push(id);
    }
    Ok(payload(json!({"files": ids}), &[]))
}

fn file_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let ids = gids(ctx.arg("fileIds"));
    let state = mutation.state();
    let errors: Vec<UserError> = ids
        .iter()
        .enumerate()
        .filter(|(_, id)| {
            !matches!(
                state.type_of(id),
                Some("MediaImage" | "Video" | "ExternalVideo" | "Model3d" | "GenericFile")
            )
        })
        .map(|(index, id)| {
            UserError::new(
                &["fileIds", &index.to_string()],
                format!("File id {id} does not exist."),
            )
            .code("FILE_DOES_NOT_EXIST")
        })
        .collect();
    if !errors.is_empty() {
        return Ok(payload(json!({"deletedFileIds": null}), &errors));
    }
    let state = mutation.state_mut();
    for id in &ids {
        for product in products_with(state, id) {
            set_reference(state, &product, id, false);
        }
        state.delete(id, "files/delete");
    }
    Ok(payload(json!({"deletedFileIds": ids}), &[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_type_and_the_name_of_a_source() {
        assert_eq!(
            content_type_of("https://cdn.example/a/b.JPG?v=1", "b.JPG"),
            "IMAGE"
        );
        assert_eq!(
            content_type_of("https://www.youtube.com/watch?v=x", "watch"),
            "EXTERNAL_VIDEO"
        );
        assert_eq!(
            content_type_of("https://cdn.example/manual.pdf", "manual.pdf"),
            "FILE"
        );
        assert_eq!(last_segment("https://cdn.example/a/b.png?v=2"), "b.png");
        assert!(!valid_source("ftp://x/y") && !valid_source("not a url"));
    }
}
