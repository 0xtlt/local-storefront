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

use crate::admin::graphql::{FieldContext, FieldError};
use crate::admin::resolve::{Mutation, Registry, UserError, payload};
use crate::admin::state::{Blob, ShopState, gid_number};

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

/// The width and height of an image, from the header of its bytes: PNG, JPEG, GIF and WebP
/// (lossy, lossless and extended). `None` for another format or bytes too short to say.
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be16 = |at: usize| {
        Some(u32::from(u16::from_be_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let le16 = |at: usize| {
        Some(u32::from(u16::from_le_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let le24 = |at: usize| {
        let b = bytes.get(at..at + 3)?;
        Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
    };
    let be32 = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let size = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        // The IHDR chunk comes first: its width and height, big-endian.
        (bytes.get(12..16)? == b"IHDR").then_some(())?;
        (be32(16)?, be32(20)?)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        (le16(6)?, le16(8)?)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12)? == b"WEBP" {
        match bytes.get(12..16)? {
            b"VP8 " => (le16(26)? & 0x3fff, le16(28)? & 0x3fff),
            b"VP8L" => {
                let bits = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
                ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
            }
            b"VP8X" => (le24(24)? + 1, le24(27)? + 1),
            _ => return None,
        }
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        // The segments up to the first start of frame (SOF0 to SOF15 but DHT, JPG and DAC).
        let mut at = 2;
        loop {
            while *bytes.get(at)? != 0xff {
                at += 1;
            }
            while *bytes.get(at)? == 0xff {
                at += 1;
            }
            let marker = *bytes.get(at)?;
            at += 1;
            if matches!(marker, 0xd8 | 0x01 | 0xd0..=0xd7) {
                continue;
            }
            let length = be16(at)? as usize;
            if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                break (be16(at + 5)?, be16(at + 3)?);
            }
            at += length;
        }
    } else {
        return None;
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
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
            crate::admin::webhooks::sign::new_id()
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

    /// The headers of the image formats: their sizes are read, others are not.
    #[test]
    fn reads_the_size_of_an_image_from_its_header() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(640u32.to_be_bytes());
        png.extend(480u32.to_be_bytes());
        png.extend([8, 6, 0, 0, 0]);
        assert_eq!(image_size(&png), Some((640, 480)));
        let mut gif = b"GIF89a".to_vec();
        gif.extend(300u16.to_le_bytes());
        gif.extend(200u16.to_le_bytes());
        assert_eq!(image_size(&gif), Some((300, 200)));
        // A JPEG: SOI, an APP0 segment, then SOF0 (precision, height, width).
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x4a, 0x46];
        jpeg.extend([0xff, 0xc0, 0x00, 0x11, 0x08]);
        jpeg.extend(1080u16.to_be_bytes());
        jpeg.extend(1920u16.to_be_bytes());
        jpeg.extend([0x03; 12]);
        assert_eq!(image_size(&jpeg), Some((1920, 1080)));
        // WebP: lossy (VP8), lossless (VP8L), extended (VP8X).
        let webp = |chunk: &[u8], body: &[u8]| {
            let mut bytes = b"RIFF\0\0\0\0WEBP".to_vec();
            bytes.extend(chunk);
            bytes.extend([0; 4]);
            bytes.extend(body);
            bytes
        };
        let mut lossy = vec![0, 0, 0, 0x9d, 0x01, 0x2a];
        lossy.extend(800u16.to_le_bytes());
        lossy.extend(600u16.to_le_bytes());
        assert_eq!(image_size(&webp(b"VP8 ", &lossy)), Some((800, 600)));
        let bits: u32 = (100 - 1) | ((50 - 1) << 14);
        let mut lossless = vec![0x2f];
        lossless.extend(bits.to_le_bytes());
        assert_eq!(image_size(&webp(b"VP8L", &lossless)), Some((100, 50)));
        let mut extended = vec![0; 4];
        extended.extend([0x1f, 0x03, 0x00, 0x57, 0x02, 0x00]);
        assert_eq!(image_size(&webp(b"VP8X", &extended)), Some((800, 600)));
        assert_eq!(image_size(b"%PDF-1.7"), None);
        assert_eq!(image_size(&png[..20]), None);
    }

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
