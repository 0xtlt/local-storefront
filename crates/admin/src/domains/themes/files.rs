//! The files of a theme: the local theme's from its directory with the Admin API's overlay on
//! top, another theme's from the shop's blobs; `OnlineStoreTheme.files(filenames:, first:)`.
//!
//! A file is an `OnlineStoreThemeFile`: its `filename` (`templates/index.json`), its `body` (the
//! text of a UTF-8 file, `OnlineStoreThemeFileBodyText`, else its bytes in Base64,
//! `OnlineStoreThemeFileBodyBase64`), `checksumMd5` (the hex MD5 of its bytes, as the REST Asset
//! API's `checksum`), `size`, `contentType` (by extension: `text/x-liquid` for Liquid, as the
//! Asset API answers), and dates: a file of the directory was created with the theme and
//! updated when the file system says; a file the Admin API wrote, when it did.
//!
//! `filenames` takes `*` for any characters ("Use '*' to match zero or more characters", 50 at
//! most); a name without `*` that no file has is one of the connection's `userErrors`
//! (`{code: NOT_FOUND, filename}`).

use std::path::Path;
use std::sync::Arc;

use md5::{Digest, Md5};
use serde_json::{Map, Value, json};

use super::{FILES, is_local, root};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Obj, Registry, ShopResolver};
use crate::state::{Blob, Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .field("OnlineStoreTheme", "files", theme_files)
        .field("OnlineStoreThemeFileConnection", "userErrors", read_errors);
}

/// A file of a theme.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeFile {
    pub filename: String,
    pub bytes: Arc<[u8]>,
    pub created_at: String,
    pub updated_at: String,
}

impl ThemeFile {
    /// The `OnlineStoreThemeFile` of the file.
    pub fn to_value(&self) -> Value {
        let body = match std::str::from_utf8(&self.bytes) {
            Ok(text) => json!({"__typename": "OnlineStoreThemeFileBodyText", "content": text}),
            Err(_) => json!({
                "__typename": "OnlineStoreThemeFileBodyBase64",
                "contentBase64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &self.bytes),
            }),
        };
        json!({
            "filename": self.filename,
            "body": body,
            "checksumMd5": md5_hex(&self.bytes),
            "contentType": content_type(&self.filename),
            "size": self.bytes.len().to_string(),
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        })
    }

    /// The `OnlineStoreThemeFileOperationResult` of a write of the file.
    pub fn operation_result(&self) -> Value {
        json!({
            "filename": self.filename,
            "checksumMd5": md5_hex(&self.bytes),
            "size": self.bytes.len().to_string(),
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        })
    }
}

/// The hex MD5 digest of bytes.
pub fn md5_hex(bytes: &[u8]) -> String {
    Md5::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The content type of a theme file, by its extension.
pub fn content_type(filename: &str) -> &'static str {
    let extension = filename
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "liquid" => "text/x-liquid",
        "json" => "application/json",
        "css" => "text/css",
        "js" | "mjs" => "application/javascript",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Why a filename is not one of a theme: it is in one of the theme's directories, without `..`.
pub fn check_filename(filename: &str) -> Result<(), String> {
    let Some((directory, rest)) = filename.split_once('/') else {
        return Err(format!(
            "{filename} is not in a theme directory ({})",
            lsf_core::theme::THEME_DIRECTORIES.join(", ")
        ));
    };
    if !lsf_core::theme::THEME_DIRECTORIES.contains(&directory) {
        return Err(format!(
            "{filename} is not in a theme directory ({})",
            lsf_core::theme::THEME_DIRECTORIES.join(", ")
        ));
    }
    let nested_allowed = directory == "templates"
        && (rest.starts_with("customers/") || rest.starts_with("metaobject/"));
    if rest.is_empty()
        || rest
            .split('/')
            .any(|part| part.is_empty() || part == ".." || part == ".")
        || (rest.contains('/') && !nested_allowed)
    {
        return Err(format!("{filename} is not a valid theme filename"));
    }
    Ok(())
}

/// The bytes a file entry of a theme record holds: `None` for a deleted one.
pub fn entry_bytes(state: &ShopState, entry: &Value) -> Option<Vec<u8>> {
    let key = entry.get("blob")?.as_str()?;
    state.blob(key).map(|blob| blob.bytes.to_vec())
}

fn entry_file(state: &ShopState, filename: &str, entry: &Value) -> Option<ThemeFile> {
    let key = entry.get("blob")?.as_str()?;
    let blob = state.blob(key)?;
    Some(ThemeFile {
        filename: filename.to_string(),
        bytes: blob.bytes.clone(),
        created_at: entry
            .get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        updated_at: entry
            .get("updatedAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// The files of the directory, by filename, sorted.
fn directory_files(root: &Path, created: &str) -> Vec<ThemeFile> {
    let mut files = Vec::new();
    for directory in lsf_core::theme::THEME_DIRECTORIES {
        walk(root, &root.join(directory), created, &mut files);
    }
    files.sort_by(|a, b| a.filename.cmp(&b.filename));
    files
}

fn walk(root: &Path, directory: &Path, created: &str, files: &mut Vec<ThemeFile>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            walk(root, &path, created, files);
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let filename = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let updated = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .map(|modified| value::date_time(chrono::DateTime::<chrono::Utc>::from(modified)))
            .unwrap_or_else(|| created.to_string());
        files.push(ThemeFile {
            filename,
            bytes: Arc::from(bytes),
            created_at: created.to_string(),
            updated_at: updated,
        });
    }
}

fn theme_created(theme: &Record) -> String {
    theme
        .get("createdAt")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every file of a theme, by filename.
pub fn list(state: &ShopState, theme: &Record) -> Vec<ThemeFile> {
    let entries = theme
        .get(FILES)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut files: Vec<ThemeFile> = if is_local(theme) {
        root(state)
            .map(|root| directory_files(&root, &theme_created(theme)))
            .unwrap_or_default()
            .into_iter()
            .filter(|file| !entries.contains_key(&file.filename))
            .collect()
    } else {
        Vec::new()
    };
    files.extend(
        entries
            .iter()
            .filter_map(|(filename, entry)| entry_file(state, filename, entry)),
    );
    files.sort_by(|a, b| a.filename.cmp(&b.filename));
    files
}

/// One file of a theme.
pub fn read(state: &ShopState, theme: &Record, filename: &str) -> Option<ThemeFile> {
    if let Some(entry) = theme.get(FILES).and_then(|files| files.get(filename)) {
        return entry_file(state, filename, entry);
    }
    if !is_local(theme) {
        return None;
    }
    let root = root(state)?;
    check_filename(filename).ok()?;
    let path = root.join(filename);
    let bytes = std::fs::read(&path).ok()?;
    let created = theme_created(theme);
    let updated = std::fs::metadata(&path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(|modified| value::date_time(chrono::DateTime::<chrono::Utc>::from(modified)))
        .unwrap_or_else(|| created.clone());
    Some(ThemeFile {
        filename: filename.to_string(),
        bytes: Arc::from(bytes),
        created_at: created,
        updated_at: updated,
    })
}

/// Writes a file of a theme: its bytes kept as a blob of the shop (by their digest: two themes
/// with the same file share it), its entry in the theme's record. Answers the file.
pub fn put(state: &mut ShopState, theme_id: &str, filename: &str, bytes: Vec<u8>) -> ThemeFile {
    let digest = md5_hex(&bytes);
    let key = format!("theme-files/{digest}");
    let now = state.timestamp();
    let created = state
        .get(theme_id)
        .and_then(|theme| theme.get(FILES)?.get(filename)?.get("createdAt")?.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| now.clone());
    if state.blob(&key).is_none() {
        state.put_blob(&key, Blob::new(bytes.clone(), content_type(filename)));
    }
    let entry = json!({
        "blob": key,
        "checksumMd5": digest,
        "size": bytes.len(),
        "contentType": content_type(filename),
        "createdAt": created,
        "updatedAt": now,
    });
    if let Some(theme) = state.get_mut(theme_id) {
        let mut files = theme
            .get(FILES)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        files.insert(filename.to_string(), entry);
        theme.insert(FILES.into(), Value::Object(files));
    }
    state.touch(theme_id);
    ThemeFile {
        filename: filename.to_string(),
        bytes: Arc::from(bytes),
        created_at: created,
        updated_at: now,
    }
}

/// Deletes a file of a theme: from the overlay, and, for a file of the directory, as deleted.
pub fn remove(state: &mut ShopState, theme_id: &str, filename: &str) {
    let on_disk = state
        .get(theme_id)
        .filter(|theme| is_local(theme))
        .and_then(|_| root(state))
        .is_some_and(|root| root.join(filename).is_file());
    if let Some(theme) = state.get_mut(theme_id) {
        let mut files: Map<String, Value> = theme
            .get(FILES)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if on_disk {
            files.insert(filename.to_string(), json!({"deleted": true}));
        } else {
            files.remove(filename);
        }
        theme.insert(FILES.into(), Value::Object(files));
    }
    state.touch(theme_id);
}

/// Whether `filename` matches a pattern with `*` wildcards.
pub fn matches(pattern: &str, filename: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == filename;
    }
    let mut rest = filename;
    for (index, part) in parts.iter().enumerate() {
        if index == 0 {
            let Some(after) = rest.strip_prefix(part) else {
                return false;
            };
            rest = after;
        } else if index == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

/// `OnlineStoreTheme.files(filenames:, first:, after:)`.
fn theme_files(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(theme) = r.map_of(object).cloned() else {
        return Ok(Output::Null);
    };
    let patterns: Option<Vec<String>> =
        ctx.arg("filenames").and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        });
    let mut files = list(r.state(), &theme);
    let mut missing = Vec::new();
    if let Some(patterns) = &patterns {
        files.retain(|file| {
            patterns
                .iter()
                .any(|pattern| matches(pattern, &file.filename))
        });
        for pattern in patterns.iter().filter(|pattern| !pattern.contains('*')) {
            if !files.iter().any(|file| &file.filename == pattern) {
                missing.push(json!({"code": "NOT_FOUND", "filename": pattern}));
            }
        }
    }
    let items: Vec<Value> = files.iter().map(ThemeFile::to_value).collect();
    let output = r.connection(ctx, items)?;
    // The read errors go with the connection: its `userErrors` field reads them.
    Ok(match output {
        Output::Object(Obj::Inline(inline)) => {
            let mut value = inline.value.clone();
            value.insert("lsfReadErrors".into(), json!(missing));
            Output::Object(if inline.reported {
                Obj::inline(&inline.type_name, value)
            } else {
                Obj::built(&inline.type_name, value)
            })
        }
        other => other,
    })
}

/// `OnlineStoreThemeFileConnection.userErrors`: the filenames asked for that no file has.
fn read_errors(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let errors = r
        .map_of(object)
        .and_then(|connection| connection.get("lsfReadErrors"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    r.convert(ctx, &errors, true)
}
