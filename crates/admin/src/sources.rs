//! The bytes a mutation reads from a URL: `themeCreate(source:)`, a `URL` body of
//! `themeFilesUpsert`, `urlRedirectImportCreate(url:)`.
//!
//! - **lsf's own staged uploads** (`/__lsf/admin/uploads/…`, the `resourceUrl` of a target, or
//!   its key) are read from the shop's blobs, never over HTTP: lsf does not call itself.
//! - **Any other URL is fetched before the request locks its shop** ([`prefetch`]): a mutation
//!   runs under the shop's lock, and a download within it held the shop (and every request
//!   waiting on it, the storefront's carts and checkouts included) for as long as the source
//!   took, 10 s each, 50 of them for one `themeFilesUpsert`. The api layer finds the URLs of the
//!   request ([`urls`]: the three mutations, their arguments from the query and its variables),
//!   fetches them together (8 at a time, each within 10 s, all within 30 s), and gives what it
//!   fetched to the request ([`Fetched`]); the mutation reads it from there ([`bytes`]). A URL
//!   the request did not name where [`urls`] looks (a line of a bulk operation) is not fetched,
//!   and says so.
//! - A source has [`MAX_SOURCE_BYTES`] at most: one byte more is read, so that a larger one is
//!   refused (`ZIP_TOO_LARGE`), not cut.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::graphql::ast::{Document, Selection, SelectionSet, ValueKind};
use crate::graphql::{OperationKind, Request};
use crate::resolve::Mutation;
use crate::state::ShopState;

/// The most bytes a source fetched from a URL may have: "May not be used to fetch a file bigger
/// than 50MB" (`ZIP_TOO_LARGE`).
pub const MAX_SOURCE_BYTES: usize = 50 << 20;

/// How long one source may take.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the sources of one request may take, all together.
pub const REQUEST_FETCH_BUDGET: Duration = Duration::from_secs(30);

/// How many sources of a request are fetched at once.
const PARALLEL_FETCHES: usize = 8;

/// The mutations that read a URL, the only ones whose URLs are fetched.
pub const READERS: [&str; 3] = ["themeCreate", "themeFilesUpsert", "urlRedirectImportCreate"];

/// Why the bytes at a URL could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// More than [`MAX_SOURCE_BYTES`] were there.
    TooLarge,
    /// Not there, not an upload nor an `http(s)` URL, not reachable: why, for the message.
    Unreadable(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::TooLarge => {
                f.write_str("May not be used to fetch a file bigger than 50MB")
            }
            SourceError::Unreadable(why) => f.write_str(why),
        }
    }
}

/// What one source gave: its bytes, or why there are none.
pub type Outcome = Result<Arc<[u8]>, SourceError>;

/// What a request's sources gave, fetched before it locked its shop: by URL, the bytes or why
/// there are none.
#[derive(Clone, Debug, Default)]
pub struct Fetched {
    by_url: HashMap<String, Outcome>,
}

impl Fetched {
    /// What `url` gave, if it was fetched.
    pub fn get(&self, url: &str) -> Option<&Outcome> {
        self.by_url.get(url)
    }

    /// The URLs fetched.
    pub fn len(&self) -> usize {
        self.by_url.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_url.is_empty()
    }
}

/// The bytes at `url` for the mutation `m`: a staged upload of the shop, else what the request
/// fetched before it locked the shop.
pub fn bytes(m: &Mutation<'_>, url: &str) -> Result<Vec<u8>, SourceError> {
    bytes_of(m.state(), m.fetched(), url)
}

/// [`bytes`], from a shop and what its request fetched.
pub fn bytes_of(
    state: &ShopState,
    fetched: Option<&Fetched>,
    url: &str,
) -> Result<Vec<u8>, SourceError> {
    let url = url.trim();
    if let Some(blob) = crate::domains::app::uploads::bytes(state, url) {
        return Ok(blob.bytes.to_vec());
    }
    if is_upload(url) {
        return Err(SourceError::Unreadable(format!(
            "nothing was uploaded to {url}: upload the file to the target of stagedUploadsCreate first"
        )));
    }
    if !is_http(url) {
        return Err(SourceError::Unreadable(format!(
            "{url} is neither a staged upload nor an http(s) URL"
        )));
    }
    match fetched.and_then(|fetched| fetched.get(url)) {
        Some(Ok(bytes)) => Ok(bytes.to_vec()),
        Some(Err(error)) => Err(error.clone()),
        None => Err(SourceError::Unreadable(format!(
            "{url} was not fetched: lsf fetches the sources a request names before it runs (the \
             source of themeCreate, the URL bodies of themeFilesUpsert, the url of \
             urlRedirectImportCreate), not those of a bulk operation's lines"
        ))),
    }
}

/// Whether `url` is lsf's own upload endpoint, which is read from the shop, never fetched.
fn is_upload(url: &str) -> bool {
    let path = match url.find("://") {
        Some(scheme) => {
            let rest = &url[scheme + 3..];
            rest.find('/').map_or("", |slash| &rest[slash..])
        }
        None => url,
    };
    path.starts_with(crate::domains::app::uploads::UPLOADS_PATH)
}

fn is_http(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The URLs the mutations of `request` read, from its query and its variables: the `source` of
/// `themeCreate`, the `value` of each `URL` body of `themeFilesUpsert`, the `url` of
/// `urlRedirectImportCreate`; only those of the fields `may_run` says the token may run, and
/// only those lsf would fetch (not its uploads). Nothing when the query names none of them,
/// without parsing it.
pub fn urls(request: &Request, may_run: &dyn Fn(&str) -> bool) -> Vec<String> {
    if !READERS.iter().any(|name| request.query.contains(name)) {
        return Vec::new();
    }
    let Ok(document) = crate::graphql::parse::parse(&request.query) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for operation in document.operations() {
        if operation.kind != OperationKind::Mutation
            || request
                .operation_name
                .as_deref()
                .is_some_and(|name| operation.name != Some(name))
        {
            continue;
        }
        let mut visited = Vec::new();
        collect(
            &document,
            &operation.selection_set,
            &request.variables,
            may_run,
            &mut visited,
            &mut found,
        );
    }
    let mut urls: Vec<String> = Vec::new();
    for url in found {
        let url = url.trim().to_string();
        if is_http(&url) && !is_upload(&url) && !urls.contains(&url) {
            urls.push(url);
        }
    }
    urls
}

/// The URLs of the root fields of a selection set, through its fragments.
fn collect<'q>(
    document: &'q Document<'q>,
    selections: &'q SelectionSet<'q>,
    variables: &Map<String, Value>,
    may_run: &dyn Fn(&str) -> bool,
    visited: &mut Vec<&'q str>,
    found: &mut Vec<String>,
) {
    for selection in &selections.items {
        match selection {
            Selection::Field(field) => {
                if !READERS.contains(&field.name) || !may_run(field.name) {
                    continue;
                }
                let argument = |name: &str| {
                    field
                        .arguments
                        .iter()
                        .find(|argument| argument.name == name)
                        .map_or(Value::Null, |argument| value_of(&argument.value, variables))
                };
                match field.name {
                    "themeCreate" => found.extend(argument("source").as_str().map(str::to_string)),
                    "urlRedirectImportCreate" => {
                        found.extend(argument("url").as_str().map(str::to_string))
                    }
                    _ => found.extend(
                        argument("files")
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|file| file.get("body"))
                            .filter(|body| body.get("type").and_then(Value::as_str) == Some("URL"))
                            .filter_map(|body| body.get("value")?.as_str().map(str::to_string)),
                    ),
                }
            }
            Selection::InlineFragment(fragment) => collect(
                document,
                &fragment.selection_set,
                variables,
                may_run,
                visited,
                found,
            ),
            Selection::FragmentSpread(spread) => {
                if visited.contains(&spread.name) {
                    continue;
                }
                visited.push(spread.name);
                if let Some(fragment) = document.fragment(spread.name) {
                    collect(
                        document,
                        &fragment.selection_set,
                        variables,
                        may_run,
                        visited,
                        found,
                    );
                }
            }
        }
    }
}

/// A value of the query as JSON, its variables replaced by theirs.
fn value_of(value: &crate::graphql::ast::Value<'_>, variables: &Map<String, Value>) -> Value {
    match &value.kind {
        ValueKind::Variable(name) => variables.get(*name).cloned().unwrap_or(Value::Null),
        ValueKind::Int(text) | ValueKind::Float(text) => {
            serde_json::from_str(text).unwrap_or(Value::Null)
        }
        ValueKind::String(text) => json!(text),
        ValueKind::Boolean(flag) => json!(flag),
        ValueKind::Null => Value::Null,
        ValueKind::Enum(name) => json!(name),
        ValueKind::List(items) => {
            Value::Array(items.iter().map(|item| value_of(item, variables)).collect())
        }
        ValueKind::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|field| (field.name.to_string(), value_of(&field.value, variables)))
                .collect(),
        ),
    }
}

/// Fetches `urls`, [`PARALLEL_FETCHES`] at a time, each within [`FETCH_TIMEOUT`] and all within
/// [`REQUEST_FETCH_BUDGET`]: what a request does before it locks its shop. No lock is held.
pub fn prefetch(urls: &[String]) -> Fetched {
    let mut fetched = Fetched::default();
    if urls.is_empty() {
        return fetched;
    }
    let deadline = Instant::now() + REQUEST_FETCH_BUDGET;
    let next = AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<Outcome>>> =
        urls.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL_FETCHES.min(urls.len()) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(url) = urls.get(index) else {
                        return;
                    };
                    let result = fetch(url, deadline);
                    if let Ok(mut slot) = results[index].lock() {
                        *slot = Some(result);
                    }
                }
            });
        }
    });
    for (url, result) in urls.iter().zip(results) {
        let result = result
            .into_inner()
            .ok()
            .flatten()
            .unwrap_or_else(|| Err(SourceError::Unreadable(format!("{url} was not fetched"))));
        fetched.by_url.insert(url.clone(), result);
    }
    fetched
}

/// The bytes at an `http(s)` URL, within [`FETCH_TIMEOUT`] and before `deadline`.
fn fetch(url: &str, deadline: Instant) -> Outcome {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(SourceError::Unreadable(format!(
            "{url} was not fetched: the sources of the request took more than {} s",
            REQUEST_FETCH_BUDGET.as_secs()
        )));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT.min(left)))
        .timeout_connect(Some(Duration::from_secs(3).min(left)))
        .http_status_as_error(false)
        .build()
        .into();
    let response = agent.get(url).call().map_err(|error| {
        SourceError::Unreadable(format!("{url} could not be downloaded: {error}"))
    })?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(SourceError::Unreadable(format!("{url} answered {status}")));
    }
    read_at_most(response.into_body().as_reader(), MAX_SOURCE_BYTES)
        .map_err(|error| SourceError::Unreadable(format!("{url} could not be read: {error}")))?
        .map(Arc::from)
        .ok_or(SourceError::TooLarge)
}

/// Everything `reader` has, or `None` when it has more than `limit` bytes.
pub fn read_at_most(reader: impl std::io::Read, limit: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(reader, limit as u64 + 1),
        &mut bytes,
    )?;
    Ok((bytes.len() <= limit).then_some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(query: &str, variables: Value) -> Request {
        let mut request = Request::new(query);
        if let Value::Object(variables) = variables {
            request.variables = variables;
        }
        request
    }

    #[test]
    fn the_urls_of_a_request_are_those_its_mutations_read() {
        let all = |_: &str| true;
        let found = urls(
            &request(
                r#"mutation($files: [OnlineStoreThemeFilesUpsertFileInput!]!, $csv: URL!) {
                    themeCreate(source: "https://example.com/theme.zip") { theme { id } }
                    themeFilesUpsert(themeId: "gid://shopify/OnlineStoreTheme/1", files: $files) { job { id } }
                    ...Import
                }
                fragment Import on Mutation { urlRedirectImportCreate(url: $csv) { urlRedirectImport { id } } }"#,
                json!({
                    "files": [
                        {"filename": "assets/a.css", "body": {"type": "URL", "value": "https://example.com/a.css"}},
                        {"filename": "assets/b.css", "body": {"type": "TEXT", "value": "https://example.com/not-a-source"}},
                        {"filename": "assets/c.css", "body": {"type": "URL", "value": "http://127.0.0.1:9292/__lsf/admin/uploads/shop/tmp/1/c.css"}},
                        {"filename": "assets/d.css", "body": {"type": "URL", "value": "https://example.com/a.css"}},
                    ],
                    "csv": "https://example.com/redirects.csv",
                }),
            ),
            &all,
        );
        assert_eq!(
            found,
            [
                "https://example.com/theme.zip",
                "https://example.com/a.css",
                "https://example.com/redirects.csv",
            ]
        );
        // Not a field the token may run, not a mutation: nothing.
        let none = |_: &str| false;
        assert!(
            urls(
                &request(
                    r#"mutation { themeCreate(source: "https://example.com/t.zip") { theme { id } } }"#,
                    json!({})
                ),
                &none
            )
            .is_empty()
        );
        assert!(
            urls(
                &request(r#"query { themeCreate: shop { name } }"#, json!({})),
                &all
            )
            .is_empty()
        );
    }

    #[test]
    fn an_upload_is_never_fetched_and_an_unfetched_url_says_why() {
        let (store, _) = lsf_core::store::load::load(
            &lsf_core::store::load::DataSource::Demo,
            &Default::default(),
        );
        let shop = ShopState::seeded(&store, None);
        let upload = bytes_of(
            &shop,
            None,
            "http://127.0.0.1:9292/__lsf/admin/uploads/my-shop.myshopify.com/tmp/1/x.zip",
        );
        assert!(
            matches!(&upload, Err(SourceError::Unreadable(why)) if why.contains("nothing was uploaded")),
            "{upload:?}"
        );
        let unfetched = bytes_of(
            &shop,
            Some(&Fetched::default()),
            "https://example.com/x.zip",
        );
        assert!(
            matches!(&unfetched, Err(SourceError::Unreadable(why)) if why.contains("was not fetched")),
            "{unfetched:?}"
        );
        assert_eq!(
            read_at_most(&[0u8; 10][..], 10).unwrap().map(|b| b.len()),
            Some(10)
        );
        assert_eq!(read_at_most(&[0u8; 11][..], 10).unwrap(), None);
    }
}
