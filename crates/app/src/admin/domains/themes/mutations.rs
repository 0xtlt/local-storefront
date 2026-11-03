//! The mutations of themes and of their files.
//!
//! As shopify.dev's reference of 2026-10 describes them (and the codes of their error enums,
//! whose descriptions are the only wording Shopify publishes; where none fits, the message is
//! lsf's):
//!
//! - **`themeCreate(source:, name:, role:)`**: "Creates a theme from an external URL or staged
//!   upload … a ZIP file". The role is `UNPUBLISHED` by default, `DEVELOPMENT` allowed ("Only
//!   UNPUBLISHED and DEVELOPMENT roles are permitted": `INVALID_THEME_ROLE_FOR_THEME_CREATION`).
//!   The ZIP is read at once (Shopify processes it in the background, `processing: true` until
//!   done; lsf's is done when the mutation answers): `INVALID_ZIP` ("Must be a zip file"),
//!   `ZIP_IS_EMPTY`, `ZIP_TOO_LARGE` (50 MB, "May not be used to fetch a file bigger than
//!   50MB"); an archive without `layout/theme.liquid` is no theme (`INVALID_ZIP`, lsf's choice:
//!   Shopify would fail its processing). Its files outside the theme's directories (a
//!   `README.md`) are left out. The name is the one given, else the theme's `theme_info`.
//!   `themes/create`.
//! - **`themeUpdate(id:, input: {name})`**: `TOO_LONG` past 50 characters (the admin's limit),
//!   `INVALID` for a blank name. `themes/update`.
//! - **`themeDelete(id:)`**: the main theme cannot be deleted ("if the theme is the last
//!   published theme", the REST reference says; the payload's only code is `NOT_FOUND`, so the
//!   error has none). Nor can the local theme, published or not: it is the directory, which
//!   has no copy in the shop to publish again. `themes/delete`.
//! - **`themeDuplicate(id:, name:)`**: an `UNPUBLISHED` copy with every file, named
//!   "Copy of <name>" unless given one (the admin's). `themes/create`.
//! - **`themePublish(id:)`**: the theme becomes `MAIN` and the main one `UNPUBLISHED` ("There
//!   can only be one main theme at any time"). `themes/publish` for the theme published,
//!   `themes/update` for the one it replaces. The storefront serves the main theme.
//! - **`themeFilesUpsert(themeId:, files:)`**: "a maximum of 50 files in a single request"
//!   (`LESS_THAN_OR_EQUAL_TO`), one input per filename (`DUPLICATE_FILE_INPUT`), each in a
//!   theme directory and, for JSON, valid JSON (comments allowed, as Shopify's JSON templates):
//!   `FILE_VALIDATION_ERROR`. A `TEXT` body is the file, a `BASE64` one its bytes, a `URL` one is
//!   fetched (a staged upload, or downloaded): the reference's example answers the files of a
//!   `URL` body through its `job`, not in `upsertedThemeFiles`, and so does lsf, its job done.
//!   Nothing is written when a file is refused (lsf's choice: Shopify does not say).
//! - **`themeFilesCopy(themeId:, files: [{srcFilename, dstFilename}])`**: "Copying to existing
//!   theme files will overwrite them"; a source that is not there is `NOT_FOUND`.
//! - **`themeFilesDelete(themeId:, files:)`**: a file that is not there is `NOT_FOUND`;
//!   `layout/theme.liquid`, without which a theme does not render, cannot be deleted.
//!
//! The files mutations' errors are `OnlineStoreThemeFilesUserErrors`, with the `filename` they
//! are about. A theme `LOCKED` or `ARCHIVED` takes no file (`ACCESS_DENIED`: "Customization and
//! publishing are restricted"). Shopify has no webhook topic for theme files ("Does not occur
//! when theme files are updated"): their changes are `theme_files/update` events, which no
//! subscription can name.

use serde_json::{Value, json};

use super::files::{self, ThemeFile, check_filename};
use super::{is_local, local_theme_gid, theme_name, theme_of};
use crate::admin::graphql::{FieldContext, FieldError};
use crate::admin::resolve::{Mutation, Registry, UserError, done_job, payload};
use crate::admin::sources::{self, SourceError};
use crate::admin::state::{ShopState, value};

/// The most files one `themeFilesUpsert` (or copy, or delete) takes.
pub const MAX_FILES: usize = 50;

/// The longest theme name.
pub const MAX_NAME: usize = 50;

/// The largest theme archive: "May not be used to fetch a file bigger than 50MB".
pub const MAX_ZIP_BYTES: usize = 50 << 20;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("themeCreate", theme_create)
        .mutation("themeUpdate", theme_update)
        .mutation("themeDelete", theme_delete)
        .mutation("themeDuplicate", theme_duplicate)
        .mutation("themePublish", theme_publish)
        .mutation("themeFilesUpsert", files_upsert)
        .mutation("themeFilesCopy", files_copy)
        .mutation("themeFilesDelete", files_delete);
}

fn not_found(field: &str) -> UserError {
    UserError::new(&[field], "Theme does not exist").code("NOT_FOUND")
}

/// A new theme record: `role`, `name`, no files yet.
fn new_theme(state: &mut ShopState, name: &str, role: &str) -> String {
    let id = state.new_gid("OnlineStoreTheme");
    let number = crate::admin::state::gid_number(&id);
    let now = state.timestamp();
    state.insert(value::object(super::record(
        number, name, role, false, &now,
    )));
    id
}

fn theme_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let refuse = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"theme": null}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    let role = ctx.arg_str("role").unwrap_or("UNPUBLISHED").to_string();
    if role != "UNPUBLISHED" && role != "DEVELOPMENT" {
        return refuse(
            &["role"],
            "Invalid theme role for theme creation.",
            "INVALID_THEME_ROLE_FOR_THEME_CREATION",
        );
    }
    let source = ctx.arg_str("source").unwrap_or_default().trim().to_string();
    let bytes = match sources::bytes(m, &source) {
        Ok(bytes) => bytes,
        Err(SourceError::TooLarge) => {
            return refuse(
                &["source"],
                "May not be used to fetch a file bigger than 50MB",
                "ZIP_TOO_LARGE",
            );
        }
        Err(why) => {
            return refuse(
                &["source"],
                &format!("Must be a zip file: {why}"),
                "INVALID_ZIP",
            );
        }
    };
    if bytes.len() > MAX_ZIP_BYTES {
        return refuse(
            &["source"],
            "May not be used to fetch a file bigger than 50MB",
            "ZIP_TOO_LARGE",
        );
    }
    let archive = match super::zip::files(&bytes) {
        Ok(files) => files,
        Err(super::zip::ZipError::Empty) => {
            return refuse(&["source"], "Zip is empty.", "ZIP_IS_EMPTY");
        }
        Err(super::zip::ZipError::TooLarge) => {
            return refuse(
                &["source"],
                "May not be used to fetch a file bigger than 50MB",
                "ZIP_TOO_LARGE",
            );
        }
        Err(super::zip::ZipError::Invalid(why)) => {
            return refuse(
                &["source"],
                &format!("Must be a zip file: {why}"),
                "INVALID_ZIP",
            );
        }
    };
    let theme_files: Vec<(String, Vec<u8>)> = archive
        .into_iter()
        .filter(|(path, _)| check_filename(path).is_ok())
        .collect();
    if !theme_files
        .iter()
        .any(|(path, _)| path == "layout/theme.liquid")
    {
        return refuse(
            &["source"],
            "Must be a zip file of a theme: it has no layout/theme.liquid",
            "INVALID_ZIP",
        );
    }
    let name = ctx
        .arg_str("name")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            theme_files
                .iter()
                .find(|(path, _)| path == "config/settings_schema.json")
                .and_then(|(_, bytes)| theme_name(&String::from_utf8_lossy(bytes)))
        })
        .unwrap_or_else(|| "Untitled".to_string());
    let state = m.state_mut();
    let id = new_theme(state, &name, &role);
    for (path, bytes) in theme_files {
        files::put(state, &id, &path, bytes);
    }
    state.emit("themes/create", &id);
    Ok(payload(json!({"theme": id}), &[]))
}

fn theme_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if theme_of(m.state(), &id).is_none() {
        return Ok(payload(json!({"theme": null}), &[not_found("id")]));
    }
    let name = ctx
        .arg("input")
        .and_then(|input| input.get("name"))
        .and_then(Value::as_str)
        .map(|name| name.trim().to_string());
    if let Some(name) = &name {
        let error = if name.is_empty() {
            Some(UserError::new(&["input", "name"], "Name can't be blank").code("INVALID"))
        } else if name.chars().count() > MAX_NAME {
            Some(
                UserError::new(
                    &["input", "name"],
                    format!("Name is too long (maximum is {MAX_NAME} characters)"),
                )
                .code("TOO_LONG"),
            )
        } else {
            None
        };
        if let Some(error) = error {
            return Ok(payload(json!({"theme": null}), &[error]));
        }
    }
    let state = m.state_mut();
    if let Some(name) = name
        && let Some(theme) = state.get_mut(&id)
    {
        theme.insert("name".into(), json!(name));
        // The storefront's `Shopify.theme.name` follows it from now on.
        theme.insert(super::RENAMED.into(), json!(true));
    }
    state.touch(&id);
    state.emit("themes/update", &id);
    Ok(payload(json!({"theme": id}), &[]))
}

fn theme_delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let Some(theme) = theme_of(m.state(), &id) else {
        return Ok(payload(json!({"deletedThemeId": null}), &[not_found("id")]));
    };
    if theme.get("role").and_then(Value::as_str) == Some("MAIN") {
        let error = UserError::new(
            &["id"],
            "The main theme can't be deleted: publish another theme first",
        );
        return Ok(payload(json!({"deletedThemeId": null}), &[error]));
    }
    // The theme directory has no copy in the shop: deleted, nothing could publish it again
    // until a reset. It is refused as the main theme is, whatever its role now.
    if is_local(&theme) {
        let error = UserError::new(
            &["id"],
            "The local theme can't be deleted: it is the theme directory lsf serves, which \
             themePublish makes the main theme again",
        );
        return Ok(payload(json!({"deletedThemeId": null}), &[error]));
    }
    m.state_mut().delete(&id, "themes/delete");
    Ok(payload(json!({"deletedThemeId": id}), &[]))
}

fn theme_duplicate(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let Some(theme) = theme_of(m.state(), &id) else {
        return Ok(payload(json!({"newTheme": null}), &[not_found("id")]));
    };
    let name = ctx
        .arg_str("name")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "Copy of {}",
                theme
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )
        });
    let copied = files::list(m.state(), &theme);
    let state = m.state_mut();
    let new = new_theme(state, &name, "UNPUBLISHED");
    for file in copied {
        files::put(state, &new, &file.filename, file.bytes.to_vec());
    }
    state.emit("themes/create", &new);
    Ok(payload(json!({"newTheme": new}), &[]))
}

fn theme_publish(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let Some(theme) = theme_of(m.state(), &id) else {
        return Ok(payload(json!({"theme": null}), &[not_found("id")]));
    };
    if theme.get("role").and_then(Value::as_str) == Some("MAIN") {
        return Ok(payload(json!({"theme": id}), &[]));
    }
    let previous = super::main_theme(m.state());
    let state = m.state_mut();
    if let Some(previous) = &previous {
        if let Some(record) = state.get_mut(previous) {
            record.insert("role".into(), json!("UNPUBLISHED"));
        }
        state.touch(previous);
        state.emit("themes/update", previous);
    }
    if let Some(record) = state.get_mut(&id) {
        record.insert("role".into(), json!("MAIN"));
    }
    state.touch(&id);
    state.emit("themes/publish", &id);
    if id != local_theme_gid() {
        m.note(
            "themePublish: the storefront serves the published theme's files, not the theme \
             directory, until the local theme (gid://shopify/OnlineStoreTheme/1) is published again",
        );
    }
    Ok(payload(json!({"theme": id}), &[]))
}

// ---- files -------------------------------------------------------------------------------

/// An `OnlineStoreThemeFilesUserErrors`: with the `filename` it is about.
fn file_error(field: &[&str], message: &str, code: &str, filename: Option<&str>) -> Value {
    let mut error = UserError::new(field, message).code(code).to_json();
    error["filename"] = json!(filename);
    error
}

fn files_payload(result_field: &str, results: Value, errors: Vec<Value>, job: Value) -> Value {
    let mut answer = json!({"userErrors": errors});
    answer[result_field] = results;
    if result_field == "upsertedThemeFiles" {
        answer["job"] = job;
    }
    answer
}

/// The theme a files mutation writes to, or the payload that says why not.
fn writable_theme(
    m: &Mutation<'_>,
    ctx: &FieldContext<'_>,
    result_field: &str,
    count: usize,
) -> Result<String, Value> {
    let id = ctx.arg_str("themeId").unwrap_or_default().to_string();
    let refuse = |error: Value| files_payload(result_field, Value::Null, vec![error], Value::Null);
    let Some(theme) = theme_of(m.state(), &id) else {
        return Err(refuse(file_error(
            &["themeId"],
            "Theme does not exist",
            "NOT_FOUND",
            None,
        )));
    };
    if matches!(
        theme.get("role").and_then(Value::as_str),
        Some("LOCKED" | "ARCHIVED")
    ) {
        return Err(refuse(file_error(
            &["themeId"],
            "The theme is locked: its files can't be changed",
            "ACCESS_DENIED",
            None,
        )));
    }
    if count > MAX_FILES {
        return Err(refuse(file_error(
            &["files"],
            &format!("The number of files must be less than or equal to {MAX_FILES}"),
            "LESS_THAN_OR_EQUAL_TO",
            None,
        )));
    }
    Ok(id)
}

/// The errors of filenames given twice.
fn duplicates(filenames: &[String]) -> Vec<Value> {
    let mut errors = Vec::new();
    for (index, filename) in filenames.iter().enumerate() {
        if filenames[..index].contains(filename) {
            let position = index.to_string();
            errors.push(file_error(
                &["files", &position],
                &format!("{filename} is given more than once"),
                "DUPLICATE_FILE_INPUT",
                Some(filename),
            ));
        }
    }
    errors
}

/// Checks the content of a file as Shopify does on upload: a JSON file must be JSON.
fn check_content(filename: &str, bytes: &[u8]) -> Result<(), String> {
    if filename.ends_with(".json") {
        let text = std::str::from_utf8(bytes).map_err(|_| format!("{filename} is not UTF-8"))?;
        lsf_core::json::parse_lenient(filename, text)
            .map_err(|error| format!("{filename} is not valid JSON: {error}"))?;
    }
    Ok(())
}

fn files_upsert(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let inputs: Vec<Value> = ctx
        .arg("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let id = match writable_theme(m, ctx, "upsertedThemeFiles", inputs.len()) {
        Ok(id) => id,
        Err(answer) => return Ok(answer),
    };
    let filenames: Vec<String> = inputs
        .iter()
        .map(|input| {
            input
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let mut errors = duplicates(&filenames);
    // `(filename, bytes, by URL)`.
    let mut written: Vec<(String, Vec<u8>, bool)> = Vec::new();
    for (index, (input, filename)) in inputs.iter().zip(&filenames).enumerate() {
        let position = index.to_string();
        if let Err(why) = check_filename(filename) {
            errors.push(file_error(
                &["files", &position, "filename"],
                &why,
                "FILE_VALIDATION_ERROR",
                Some(filename),
            ));
            continue;
        }
        let body = input.get("body").cloned().unwrap_or(Value::Null);
        let kind = body.get("type").and_then(Value::as_str).unwrap_or("TEXT");
        let given = body
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let bytes = match kind {
            "BASE64" => {
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, given.trim())
                    .map_err(|_| format!("The body of {filename} is not Base64"))
            }
            "URL" => sources::bytes(m, given).map_err(|error| match error {
                SourceError::TooLarge => {
                    format!("The body of {filename} at {given} is bigger than 50MB")
                }
                SourceError::Unreadable(why) => why,
            }),
            _ => Ok(given.as_bytes().to_vec()),
        };
        let bytes = match bytes.and_then(|bytes| check_content(filename, &bytes).map(|_| bytes)) {
            Ok(bytes) => bytes,
            Err(why) => {
                errors.push(file_error(
                    &["files", &position, "body", "value"],
                    &why,
                    "FILE_VALIDATION_ERROR",
                    Some(filename),
                ));
                continue;
            }
        };
        written.push((filename.clone(), bytes, kind == "URL"));
    }
    if !errors.is_empty() {
        return Ok(files_payload(
            "upsertedThemeFiles",
            Value::Null,
            errors,
            Value::Null,
        ));
    }
    let state = m.state_mut();
    let mut results = Vec::new();
    let mut fetched = false;
    for (filename, bytes, by_url) in written {
        let file = files::put(state, &id, &filename, bytes);
        if by_url {
            fetched = true;
        } else {
            results.push(file.operation_result());
        }
    }
    state.emit("theme_files/update", &id);
    let job = if fetched {
        json!(done_job(state, "theme_files_upsert"))
    } else {
        Value::Null
    };
    Ok(files_payload(
        "upsertedThemeFiles",
        Value::Array(results),
        Vec::new(),
        job,
    ))
}

fn files_copy(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let inputs: Vec<Value> = ctx
        .arg("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let id = match writable_theme(m, ctx, "copiedThemeFiles", inputs.len()) {
        Ok(id) => id,
        Err(answer) => return Ok(answer),
    };
    let Some(theme) = theme_of(m.state(), &id) else {
        return Ok(files_payload(
            "copiedThemeFiles",
            Value::Null,
            Vec::new(),
            Value::Null,
        ));
    };
    let destinations: Vec<String> = inputs
        .iter()
        .map(|input| {
            input
                .get("dstFilename")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let mut errors = duplicates(&destinations);
    let mut copies: Vec<(String, ThemeFile)> = Vec::new();
    for (index, (input, destination)) in inputs.iter().zip(&destinations).enumerate() {
        let position = index.to_string();
        let source = input
            .get("srcFilename")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(file) = files::read(m.state(), &theme, source) else {
            errors.push(file_error(
                &["files", &position, "srcFilename"],
                &format!("{source} does not exist"),
                "NOT_FOUND",
                Some(source),
            ));
            continue;
        };
        if let Err(why) = check_filename(destination) {
            errors.push(file_error(
                &["files", &position, "dstFilename"],
                &why,
                "FILE_VALIDATION_ERROR",
                Some(destination),
            ));
            continue;
        }
        copies.push((destination.clone(), file));
    }
    if !errors.is_empty() {
        return Ok(files_payload(
            "copiedThemeFiles",
            Value::Null,
            errors,
            Value::Null,
        ));
    }
    let state = m.state_mut();
    let results: Vec<Value> = copies
        .into_iter()
        .map(|(destination, file)| {
            files::put(state, &id, &destination, file.bytes.to_vec()).operation_result()
        })
        .collect();
    state.emit("theme_files/update", &id);
    Ok(files_payload(
        "copiedThemeFiles",
        Value::Array(results),
        Vec::new(),
        Value::Null,
    ))
}

fn files_delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let filenames: Vec<String> = ctx
        .arg("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let id = match writable_theme(m, ctx, "deletedThemeFiles", filenames.len()) {
        Ok(id) => id,
        Err(answer) => return Ok(answer),
    };
    let Some(theme) = theme_of(m.state(), &id) else {
        return Ok(files_payload(
            "deletedThemeFiles",
            Value::Null,
            Vec::new(),
            Value::Null,
        ));
    };
    let mut errors = duplicates(&filenames);
    let mut deleted = Vec::new();
    for (index, filename) in filenames.iter().enumerate() {
        let position = index.to_string();
        if filename == "layout/theme.liquid" {
            errors.push(file_error(
                &["files", &position],
                "layout/theme.liquid can't be deleted: a theme does not render without it",
                "FILE_VALIDATION_ERROR",
                Some(filename),
            ));
            continue;
        }
        match files::read(m.state(), &theme, filename) {
            Some(file) => deleted.push(file),
            None => errors.push(file_error(
                &["files", &position],
                &format!("{filename} does not exist"),
                "NOT_FOUND",
                Some(filename),
            )),
        }
    }
    if !errors.is_empty() {
        return Ok(files_payload(
            "deletedThemeFiles",
            Value::Null,
            errors,
            Value::Null,
        ));
    }
    let local = is_local(&theme);
    let state = m.state_mut();
    let results: Vec<Value> = deleted
        .iter()
        .map(|file| {
            files::remove(state, &id, &file.filename);
            file.operation_result()
        })
        .collect();
    state.emit("theme_files/update", &id);
    if local {
        m.note(
            "themeFilesDelete: the files of the theme directory are not deleted from disk; the \
             storefront stops serving them until DELETE /__lsf/admin",
        );
    }
    Ok(files_payload(
        "deletedThemeFiles",
        Value::Array(results),
        Vec::new(),
        Value::Null,
    ))
}
