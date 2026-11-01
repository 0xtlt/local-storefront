//! URL redirects in bulk: `urlRedirectBulkDeleteAll`, `urlRedirectBulkDeleteByIds`,
//! `urlRedirectBulkDeleteBySearch`, `urlRedirectBulkDeleteBySavedSearch`, and the imports of a
//! CSV file, `urlRedirectImportCreate` then `urlRedirectImportSubmit` (`urlRedirectImport`).
//!
//! - **Bulk deletes** "Asynchronously delete redirects in bulk": done within the request, their
//!   `job` `done`. A search is the `query` of `urlRedirects` (`path:`, `target:`, …), run as that
//!   connection runs it; a search that names a field `urlRedirects` does not have is refused
//!   (`INVALID_SEARCH_ARGUMENT`) rather than ignored, which on Shopify would delete every
//!   redirect: lsf's choice. A saved search must be one of `URL_REDIRECT`
//!   (`SAVED_SEARCH_NOT_FOUND`).
//! - **Imports**: the file is Shopify's redirect CSV (`Redirect from,Redirect to`, the export of
//!   the admin's URL redirects page), at `url`: a staged upload's `resourceUrl` (the bytes
//!   lsf keeps), or a URL lsf downloads, as Shopify does (before the shop is locked:
//!   `crate::sources`). `urlRedirectImportCreate` reads it
//!   (`count` its rows, `previewRedirects` "up to three previews"); `urlRedirectImportSubmit`
//!   creates and updates the redirects (a path already redirected is updated), counts them,
//!   and finishes: `finished`, `finishedAt`, `createdCount`, `updatedCount`, `failedCount` (a row
//!   without a path or a target, or whose target is its path). Submitting twice is
//!   `ALREADY_IMPORTED`. The file may name its columns in any order; it is read as RFC 4180 has
//!   it (a quoted cell may hold commas, line breaks and `""`), and a submit looks each path up
//!   in a map of the shop's redirects made once, so that 100,000 rows take about a second.
//!
//! Redirects have no webhook topic in 2026-10: their events are `url_redirects/<action>`, as
//! the content family's.

use serde_json::{Value, json};

use crate::domains::catalog::gids;
use crate::domains::content::navigation::set_redirect;
use crate::graphql::{self, FieldContext, FieldError, Output, Request};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, done_job, payload};
use crate::state::{Record, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("urlRedirectBulkDeleteAll", delete_all)
        .mutation("urlRedirectBulkDeleteByIds", delete_by_ids)
        .mutation("urlRedirectBulkDeleteBySearch", delete_by_search)
        .mutation("urlRedirectBulkDeleteBySavedSearch", delete_by_saved_search)
        .mutation("urlRedirectImportCreate", import_create)
        .mutation("urlRedirectImportSubmit", import_submit)
        .query("urlRedirectImport", url_redirect_import);
}

fn text<'r>(record: &'r Record, field: &str) -> &'r str {
    record
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn delete_all_of(m: &mut Mutation<'_>, ids: &[String], kind: &str) -> Value {
    let state = m.state_mut();
    for id in ids {
        state.delete(id, "url_redirects/delete");
    }
    let job = done_job(state, kind);
    payload(json!({"job": job}), &[])
}

fn delete_all(m: &mut Mutation<'_>, _ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let ids: Vec<String> = m.state().keys("UrlRedirect").map(str::to_string).collect();
    Ok(delete_all_of(m, &ids, "url_redirect_bulk_delete_all"))
}

/// `urlRedirectBulkDeleteByIds`: ids that are not redirects are skipped, as a deleted one is.
fn delete_by_ids(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let ids = gids(ctx.arg("ids"));
    if ids.is_empty() {
        let error = UserError::new(
            &["ids"],
            "You must pass one or more [`URLRedirect`] object IDs.",
        )
        .code("IDS_EMPTY");
        return Ok(payload(json!({"job": null}), &[error]));
    }
    let existing: Vec<String> = ids
        .into_iter()
        .filter(|id| m.state().type_of(id) == Some("UrlRedirect"))
        .collect();
    Ok(delete_all_of(
        m,
        &existing,
        "url_redirect_bulk_delete_by_ids",
    ))
}

/// The redirects a search of `urlRedirects` keeps, every page of them, or the fields it names
/// that the connection does not have.
fn search(m: &mut Mutation<'_>, query: &str) -> Result<Vec<String>, Vec<String>> {
    let mut ids = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut request = Request::new(
            "query($query: String!, $after: String) { urlRedirects(first: 250, after: $after, query: $query) { nodes { id } pageInfo { hasNextPage endCursor } } }",
        );
        request.variables.insert("query".into(), json!(query));
        request.variables.insert("after".into(), json!(after));
        let schema = m.schema();
        let (answer, warnings) = {
            let mut resolver = m.resolver();
            let answer = graphql::run(schema, &mut resolver, &request, None).to_json();
            (answer, resolver.extensions())
        };
        let invalid: Vec<String> = warnings
            .get("search")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|search| {
                search
                    .get("warnings")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|warning| warning.get("field")?.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .collect();
        if !invalid.is_empty() {
            return Err(invalid);
        }
        let connection = &answer["data"]["urlRedirects"];
        ids.extend(
            connection["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|node| node["id"].as_str().map(str::to_string)),
        );
        if connection["pageInfo"]["hasNextPage"] != json!(true) {
            return Ok(ids);
        }
        after = connection["pageInfo"]["endCursor"]
            .as_str()
            .map(str::to_string);
    }
}

fn delete_by_search(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let query = ctx.arg_str("search").unwrap_or_default().trim().to_string();
    let refuse = |message: String| {
        Ok(payload(
            json!({"job": null}),
            &[UserError::new(&["search"], message).code("INVALID_SEARCH_ARGUMENT")],
        ))
    };
    if query.is_empty() {
        return refuse("Search can't be blank".to_string());
    }
    match search(m, &query) {
        Ok(ids) => Ok(delete_all_of(m, &ids, "url_redirect_bulk_delete_by_search")),
        Err(fields) => refuse(format!(
            "Invalid search field for this query: {} (urlRedirects searches path, target and id)",
            fields.join(", ")
        )),
    }
}

fn delete_by_saved_search(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("savedSearchId").unwrap_or_default();
    let Some(query) =
        crate::domains::segments::saved_searches::query_for(m.state(), id, "URL_REDIRECT")
    else {
        let error = UserError::new(&["savedSearchId"], "Saved search not found")
            .code("SAVED_SEARCH_NOT_FOUND");
        return Ok(payload(json!({"job": null}), &[error]));
    };
    match search(m, &query) {
        Ok(ids) => Ok(delete_all_of(
            m,
            &ids,
            "url_redirect_bulk_delete_by_saved_search",
        )),
        Err(fields) => {
            let error = UserError::new(
                &["savedSearchId"],
                format!(
                    "The saved search's query names fields redirects do not have: {}",
                    fields.join(", ")
                ),
            )
            .code("INVALID_SAVED_SEARCH_QUERY");
            Ok(payload(json!({"job": null}), &[error]))
        }
    }
}

// ---- imports -----------------------------------------------------------------------------

/// The records of a CSV text, as RFC 4180 has them: fields separated by commas, records by
/// line breaks (`\r\n` or `\n`), a quoted field holding commas, line breaks and doubled quotes
/// (`""`). Each cell is trimmed; blank records are left out. One pass, no recursion.
fn records(csv: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = csv.chars().peekable();
    let mut end_record = |record: &mut Vec<String>, cell: &mut String| {
        record.push(std::mem::take(cell).trim().to_string());
        let fields = std::mem::take(record);
        if fields.iter().any(|field| !field.is_empty()) {
            records.push(fields);
        }
    };
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => record.push(std::mem::take(&mut cell).trim().to_string()),
            '\r' if !quoted && chars.peek() == Some(&'\n') => {}
            '\n' if !quoted => end_record(&mut record, &mut cell),
            _ => cell.push(character),
        }
    }
    end_record(&mut record, &mut cell);
    records
}

/// The `(path, target)` rows of a redirect CSV, or why it is not one.
fn rows(csv: &str) -> Result<Vec<(String, String)>, String> {
    let mut records = records(csv.trim_start_matches('\u{feff}')).into_iter();
    let header: Vec<String> = records
        .next()
        .unwrap_or_default()
        .iter()
        .map(|cell| cell.to_ascii_lowercase())
        .collect();
    let column = |names: &[&str]| {
        header
            .iter()
            .position(|cell| names.contains(&cell.as_str()))
    };
    let (Some(from), Some(to)) = (
        column(&["redirect from", "path"]),
        column(&["redirect to", "target"]),
    ) else {
        return Err(
            "The file is not a redirect CSV: its first line names the columns `Redirect from` and `Redirect to`"
                .to_string(),
        );
    };
    Ok(records
        .map(|cells| {
            (
                cells.get(from).cloned().unwrap_or_default(),
                cells.get(to).cloned().unwrap_or_default(),
            )
        })
        .collect())
}

fn import_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let url = ctx.arg_str("url").unwrap_or_default().trim().to_string();
    let refuse = |message: String| {
        Ok(payload(
            json!({"urlRedirectImport": null}),
            &[UserError::new(&["url"], message).code("NOT_FOUND")],
        ))
    };
    let bytes = match crate::sources::bytes(m, &url) {
        Ok(bytes) => bytes,
        Err(error) => return refuse(error.to_string()),
    };
    let rows = match rows(&String::from_utf8_lossy(&bytes)) {
        Ok(rows) => rows,
        Err(message) => return refuse(message),
    };
    let preview: Vec<Value> = rows
        .iter()
        .take(3)
        .map(|(path, target)| json!({"path": path, "target": target}))
        .collect();
    let state = m.state_mut();
    let id = state.insert(value::object(json!({
        "__typename": "UrlRedirectImport",
        "count": rows.len(),
        "createdCount": null,
        "updatedCount": null,
        "failedCount": null,
        "finished": false,
        "finishedAt": null,
        "previewRedirects": preview,
        "lsfRows": rows.iter().map(|(path, target)| json!([path, target])).collect::<Vec<_>>(),
    })));
    state.emit("url_redirect_imports/create", &id);
    Ok(payload(json!({"urlRedirectImport": id}), &[]))
}

fn import_submit(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = m.state();
    let Some(import) = state.get_of(&id, "UrlRedirectImport") else {
        let error = UserError::new(&["id"], "Import not found").code("NOT_FOUND");
        return Ok(payload(json!({"job": null}), &[error]));
    };
    if import.get("finished").and_then(Value::as_bool) == Some(true) {
        let error =
            UserError::new(&["id"], "The import was already submitted").code("ALREADY_IMPORTED");
        return Ok(payload(json!({"job": null}), &[error]));
    }
    let rows: Vec<(String, String)> = import
        .get("lsfRows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some((
                row.get(0)?.as_str()?.to_string(),
                row.get(1)?.as_str()?.to_string(),
            ))
        })
        .collect();
    let state = m.state_mut();
    // The redirect of each path, read once: a row looks its path up, rather than every redirect.
    let mut known: std::collections::HashMap<String, String> = state
        .all("UrlRedirect")
        .map(|redirect| {
            (
                text(redirect, "path").to_string(),
                text(redirect, "id").to_string(),
            )
        })
        .collect();
    let (mut created, mut updated, mut failed) = (0, 0, 0);
    for (path, target) in rows {
        let path = if path.starts_with('/') || path.is_empty() {
            path
        } else {
            format!("/{path}")
        };
        if path.trim_start_matches('/').is_empty() || target.is_empty() || target == path {
            failed += 1;
            continue;
        }
        let existing = known.get(&path).cloned();
        let id = set_redirect(state, existing.as_deref(), &path, &target);
        if existing.is_some() {
            updated += 1;
        } else {
            created += 1;
            known.insert(path, id);
        }
    }
    let now = state.timestamp();
    if let Some(record) = state.get_mut(&id) {
        record.insert("createdCount".into(), json!(created));
        record.insert("updatedCount".into(), json!(updated));
        record.insert("failedCount".into(), json!(failed));
        record.insert("finished".into(), json!(true));
        record.insert("finishedAt".into(), json!(now));
    }
    state.emit("url_redirect_imports/update", &id);
    let job = done_job(state, "url_redirect_import");
    Ok(payload(json!({"job": job}), &[]))
}

fn url_redirect_import(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    Ok(r.reference(id, "UrlRedirectImport")
        .map_or(Output::Null, Output::Object))
}
