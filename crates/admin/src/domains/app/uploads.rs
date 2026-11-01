//! Staged uploads: `stagedUploadsCreate` answers targets on lsf, which take the file as Shopify's
//! storage (Google Cloud Storage) does, and keep its bytes in the shop for what reads them next:
//! `bulkOperationRunMutation` (its `stagedUploadPath`), and `fileCreate` and the media inputs
//! (their `originalSource`, the target's `resourceUrl`), through [`bytes`].
//!
//! A target is what Shopify's are (shopify.dev, "Perform bulk operations": the GCS form of
//! `BULK_MUTATION_VARIABLES`), on lsf's endpoint `/__lsf/admin/uploads/<shop>`:
//!
//! - `POST` (a multipart form): `url` is the "bucket", `parameters` the form fields to post
//!   before the `file` (`Content-Type`, `success_action_status`, `acl`, `key`, `x-goog-date`,
//!   `x-goog-credential`, `x-goog-algorithm`, `x-goog-signature`, `policy`), `resourceUrl` the
//!   bucket and the key (`null` for `BULK_MUTATION_VARIABLES`, whose `key` is the
//!   `stagedUploadPath`).
//! - `PUT` (the default `httpMethod`): `url` is the object itself with the `X-Goog-*` query of a
//!   signed URL, `parameters` are `content_type` and `acl`, `resourceUrl` the object.
//!
//! The key is `tmp/<shop id>/<folder>/<uuid>/<file name>` (`bulk/…/bulk_op_vars` for bulk
//! variables), as in shopify.dev's examples. lsf checks no signature: any client of the machine
//! may upload. The bytes stay in the shop until it is reset (`DELETE /__lsf/admin`, or the store
//! data reloading). A file whose MIME type does not suit an image or video resource is refused as
//! shopify.dev's example refuses one: `"unsupported-file.xyz: (application/xyz) is not a
//! recognized format"`, its target `null`s.

use serde_json::{Value, json};

use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Blob, ShopState, gid_number};

pub fn register(registry: &mut Registry) {
    registry.mutation("stagedUploadsCreate", staged_uploads_create);
}

/// The path of lsf's upload endpoint, before the shop: `/__lsf/admin/uploads`.
pub const UPLOADS_PATH: &str = "/__lsf/admin/uploads";

/// The file name of bulk mutation variables, as Shopify's keys end.
const BULK_VARIABLES: &str = "bulk_op_vars";

/// The URL of the upload endpoint of a shop: the "bucket" of its targets.
pub fn bucket_url(origin: &str, shop: &str) -> String {
    format!("{origin}{UPLOADS_PATH}/{shop}")
}

/// The key of a staged upload from what a client gives back: the `resourceUrl`, a URL or path
/// of lsf's endpoint, or the key itself (`stagedUploadPath`, `tmp/…`).
pub fn key_of(url_or_path: &str) -> Option<String> {
    let text = url_or_path.trim();
    let path = match text.find("://") {
        Some(scheme) => {
            let rest = &text[scheme + 3..];
            rest.find('/').map_or("", |slash| &rest[slash..])
        }
        None => text,
    };
    let path = path.split(['?', '#']).next().unwrap_or_default();
    if let Some(rest) = path.strip_prefix(UPLOADS_PATH) {
        // `/<shop>/<key>`
        let rest = rest.trim_start_matches('/');
        let (_, key) = rest.split_once('/')?;
        return Some(lsf_core::urls::percent_decode(key)).filter(|key| !key.is_empty());
    }
    let key = path.trim_start_matches('/');
    key.starts_with("tmp/")
        .then(|| lsf_core::urls::percent_decode(key))
}

/// The bytes of a staged upload, by its `resourceUrl`, a URL or path of lsf's upload endpoint,
/// or its key (`stagedUploadPath`): what `fileCreate` and `bulkOperationRunMutation` read. `None`
/// when nothing was uploaded there (yet).
pub fn bytes<'s>(state: &'s ShopState, url_or_path: &str) -> Option<&'s Blob> {
    state.blob(&key_of(url_or_path)?)
}

/// Keeps what a client uploaded to a target.
pub fn store(state: &mut ShopState, key: &str, bytes: Vec<u8>, content_type: &str) {
    state.put_blob(key, Blob::new(bytes, content_type));
}

/// The folder of a resource's keys, as Shopify's keys name them (`products`, `bulk`, …).
fn folder(resource: &str) -> &'static str {
    match resource {
        "IMAGE" | "PRODUCT_IMAGE" => "products",
        "COLLECTION_IMAGE" => "collections",
        "VIDEO" => "videos",
        "MODEL_3D" => "models",
        "BULK_MUTATION_VARIABLES" => "bulk",
        "URL_REDIRECT_IMPORT" => "redirect_imports",
        "RETURN_LABEL" => "return_labels",
        "DISPUTE_FILE_UPLOAD" => "dispute_files",
        _ => "files",
    }
}

/// Whether a MIME type suits a resource: images for the image resources, videos for `VIDEO`,
/// 3D models for `MODEL_3D`; anything for the others.
fn suits(resource: &str, mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    match resource {
        "IMAGE" | "PRODUCT_IMAGE" | "COLLECTION_IMAGE" | "SHOP_IMAGE" => mime.starts_with("image/"),
        "VIDEO" => mime.starts_with("video/"),
        "MODEL_3D" => mime.starts_with("model/") || mime == "application/octet-stream",
        _ => true,
    }
}

/// The last segment of a file name a client gives (`https://x/a/b.jpg` is `b.jpg`).
fn file_name(name: &str) -> String {
    let name = name.split(['?', '#']).next().unwrap_or(name);
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim()
        .to_string()
}

fn staged_uploads_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let inputs: Vec<Value> = ctx
        .arg("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let origin = mutation.origin().to_string();
    let state = mutation.state();
    let shop = state.domain().to_string();
    let shop_id = state.shop_gid().map_or(0, gid_number);
    let date = state.now().format("%Y%m%dT%H%M%SZ").to_string();
    let day = state.now().format("%Y%m%d").to_string();
    let bucket = bucket_url(&origin, &shop);
    let mut targets = Vec::new();
    let mut errors = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let text = |field: &str| {
            input
                .get(field)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let resource = text("resource");
        let mime = text("mimeType");
        let name = file_name(&text("filename"));
        if !suits(&resource, &mime) {
            errors.push(UserError::new(
                &["input", &index.to_string(), "mimeType"],
                format!("{name}: ({mime}) is not a recognized format"),
            ));
            targets.push(json!({"url": null, "resourceUrl": null, "parameters": []}));
            continue;
        }
        let bulk = resource == "BULK_MUTATION_VARIABLES";
        let name = if bulk {
            BULK_VARIABLES.to_string()
        } else {
            name
        };
        let key = format!(
            "tmp/{shop_id}/{}/{}/{name}",
            folder(&resource),
            crate::webhooks::sign::new_id()
        );
        let credential = format!("lsf-local-storage/{day}/auto/storage/goog4_request");
        let put = input.get("httpMethod").and_then(Value::as_str) != Some("POST");
        let object = format!("{bucket}/{key}");
        targets.push(if put {
            json!({
                "url": format!(
                    "{object}?X-Goog-Algorithm=GOOG4-RSA-SHA256&X-Goog-Credential={}&X-Goog-Date={date}&X-Goog-Expires=604800&X-Goog-SignedHeaders=content-type%3Bhost&X-Goog-Signature=lsf",
                    credential.replace('/', "%2F"),
                ),
                "resourceUrl": object,
                "parameters": [
                    {"name": "content_type", "value": mime},
                    {"name": "acl", "value": "private"},
                ],
            })
        } else {
            json!({
                "url": bucket,
                "resourceUrl": if bulk { Value::Null } else { json!(object) },
                "parameters": [
                    {"name": "Content-Type", "value": mime},
                    {"name": "success_action_status", "value": "201"},
                    {"name": "acl", "value": "private"},
                    {"name": "key", "value": key},
                    {"name": "x-goog-date", "value": date},
                    {"name": "x-goog-credential", "value": credential},
                    {"name": "x-goog-algorithm", "value": "GOOG4-RSA-SHA256"},
                    {"name": "x-goog-signature", "value": "lsf"},
                    {"name": "policy", "value": "lsf"},
                ],
            })
        });
    }
    Ok(payload(json!({ "stagedTargets": targets }), &errors))
}

/// A part of a `multipart/form-data` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// The parts of a `multipart/form-data` body, by the boundary of its `Content-Type`. `None` when
/// the body is not one.
pub fn multipart(content_type: &str, body: &[u8]) -> Option<Vec<Part>> {
    let boundary = content_type.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix("boundary=")
            .map(|boundary| boundary.trim_matches('"').to_string())
    })?;
    let delimiter = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut rest = body;
    // Up to the first delimiter.
    let start = find(rest, &delimiter)?;
    rest = &rest[start + delimiter.len()..];
    loop {
        if rest.starts_with(b"--") {
            break;
        }
        rest = rest.strip_prefix(b"\r\n").unwrap_or(rest);
        let end = find(rest, &delimiter)?;
        let mut part = &rest[..end];
        part = part.strip_suffix(b"\r\n").unwrap_or(part);
        let header_end = find(part, b"\r\n\r\n")?;
        let headers = String::from_utf8_lossy(&part[..header_end]).into_owned();
        let bytes = part[header_end + 4..].to_vec();
        let mut name = None;
        let mut filename = None;
        let mut kind = None;
        for line in headers.lines() {
            let Some((header, value)) = line.split_once(':') else {
                continue;
            };
            match header.trim().to_ascii_lowercase().as_str() {
                "content-disposition" => {
                    for attribute in value.split(';').map(str::trim) {
                        if let Some(value) = attribute.strip_prefix("name=") {
                            name = Some(value.trim_matches('"').to_string());
                        } else if let Some(value) = attribute.strip_prefix("filename=") {
                            filename = Some(value.trim_matches('"').to_string());
                        }
                    }
                }
                "content-type" => kind = Some(value.trim().to_string()),
                _ => {}
            }
        }
        if let Some(name) = name {
            parts.push(Part {
                name,
                filename,
                content_type: kind,
                bytes,
            });
        }
        rest = &rest[end + delimiter.len()..];
    }
    Some(parts)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_keys_from_urls_paths_and_keys() {
        let key = "tmp/1/bulk/abc/bulk_op_vars";
        assert_eq!(key_of(key).as_deref(), Some(key));
        assert_eq!(
            key_of("http://127.0.0.1:9292/__lsf/admin/uploads/a.myshopify.com/tmp/1/products/x/a%20b.jpg?X-Goog-Date=1")
                .as_deref(),
            Some("tmp/1/products/x/a b.jpg")
        );
        assert_eq!(
            key_of("/__lsf/admin/uploads/a.myshopify.com/tmp/1/files/x/a.pdf").as_deref(),
            Some("tmp/1/files/x/a.pdf")
        );
        assert_eq!(key_of("https://cdn.example.com/a.jpg"), None);
    }

    #[test]
    fn parses_a_multipart_form() {
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"key\"\r\n\r\ntmp/1/bulk/a/bulk_op_vars\r\n--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"vars.jsonl\"\r\nContent-Type: text/jsonl\r\n\r\n{\"a\":1}\n{\"a\":2}\n\r\n--XyZ--\r\n";
        let parts = multipart("multipart/form-data; boundary=XyZ", body).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "key");
        assert_eq!(parts[0].bytes, b"tmp/1/bulk/a/bulk_op_vars");
        assert_eq!(parts[1].filename.as_deref(), Some("vars.jsonl"));
        assert_eq!(parts[1].content_type.as_deref(), Some("text/jsonl"));
        assert_eq!(parts[1].bytes, b"{\"a\":1}\n{\"a\":2}\n");
        assert!(multipart("text/plain", body).is_none());
    }
}
