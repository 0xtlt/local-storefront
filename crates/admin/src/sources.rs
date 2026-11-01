//! The bytes a mutation reads from a URL: `themeCreate(source:)`, a `URL` body of
//! `themeFilesUpsert`, `urlRedirectImportCreate(url:)`.
//!
//! - **lsf's own staged uploads** (`/__lsf/admin/uploads/…`, the `resourceUrl` of a target, or
//!   its key) are read from the shop's blobs, never over HTTP: lsf does not call itself.
//! - **A stubbed URL** ([`Stubs`]) is answered by lsf, never fetched: a backend's own URL (a
//!   theme ZIP on its CDN) that a test has no server for. The developer gives its bytes, a file
//!   (`--admin-source <url>=<file>`) or a request's body (`PUT /__lsf/admin/sources?url=`). It
//!   is the internet as the test sees it: one for every shop, kept when the shops are reset.
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
//! - **Only public addresses are fetched.** Shopify downloads a source from the internet: it
//!   can never reach the developer's machine or network. lsf refuses the same, or a backend (an
//!   app under test, sometimes a third party's) could read what the machine serves locally
//!   (an admin page, a cloud's metadata endpoint) and get it back as a theme file served on
//!   `/cdn/…`. An address is checked once resolved ([`PublicOnly`], the resolver of every
//!   connection, redirects included: what is checked is what is connected to): the loopback,
//!   private networks, link-local, unique-local, unspecified, multicast and reserved
//!   addresses are refused, IPv4 and IPv6, and IPv6 addresses that embed an IPv4 one
//!   (`::ffff:127.0.0.1`) as that one. Proxies of the environment are not used. A developer who
//!   serves a theme ZIP from their machine on purpose starts lsf with `--admin-fetch-local`
//!   ([`FetchOptions::allow_local`]). A refused source answers what an unreachable one does
//!   (`INVALID_ZIP`, `FILE_VALIDATION_ERROR`, `NOT_FOUND`), with why.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
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

/// What the sources of a server may reach.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FetchOptions {
    /// `--admin-fetch-local`: the loopback, private networks and link-local addresses too, for
    /// a developer who serves a source from their machine on purpose.
    pub allow_local: bool,
}

/// The URLs lsf answers itself, keyed by [`stub_key`].
///
/// - `configured` (`--admin-source`): the file is read at each use, so that an edit is seen at
///   once. Only the developer names a file: a request never does, or any client of the machine
///   could read a file of it back as a theme file.
/// - `registered` (`PUT /__lsf/admin/sources`): overrides a configured URL until `DELETE`.
#[derive(Debug, Default)]
pub struct Stubs {
    configured: HashMap<String, PathBuf>,
    registered: RwLock<HashMap<String, Arc<[u8]>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StubFrom {
    /// `--admin-source`.
    File(PathBuf),
    /// `PUT /__lsf/admin/sources`.
    Request,
}

/// A stubbed URL, as `GET /__lsf/admin/sources` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StubEntry {
    pub url: String,
    /// Its bytes, `None` for a file that cannot be read now.
    pub size: Option<u64>,
    pub from: StubFrom,
}

/// The URL a stub answers for, as it is looked up: `http` or `https`, the scheme and the host
/// in lowercase, without the fragment (which a client never sends), the path `/` when it has
/// none. `Err` for another scheme, a URL without a host, and lsf's own uploads, which are read
/// from the shop already.
pub fn stub_key(url: &str) -> Result<String, String> {
    let url = url.trim();
    let url = url.split('#').next().unwrap_or_default();
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(format!("`{url}` is not an http:// or https:// URL"));
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!("`{url}` is not an http:// or https:// URL"));
    }
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (host, path) = rest.split_at(split);
    if host.is_empty() {
        return Err(format!("`{url}` has no host"));
    }
    if is_upload(url) {
        return Err(format!(
            "`{url}` is a staged upload of lsf, read from the shop already: upload to it instead"
        ));
    }
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    Ok(format!("{scheme}://{}{path}", host.to_ascii_lowercase()))
}

impl Stubs {
    /// The stubs of `--admin-source`: each URL ([`stub_key`]) answered with its file. A URL that
    /// is not one is left out: the option checks them when it is read.
    pub fn configured(files: impl IntoIterator<Item = (String, PathBuf)>) -> Stubs {
        Stubs {
            configured: files
                .into_iter()
                .filter_map(|(url, file)| Some((stub_key(&url).ok()?, file)))
                .collect(),
            registered: RwLock::default(),
        }
    }

    fn registered(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Arc<[u8]>>> {
        self.registered
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn registered_mut(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Arc<[u8]>>> {
        self.registered
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The URL as it is looked up.
    pub fn register(&self, url: &str, bytes: Vec<u8>) -> Result<String, String> {
        let key = stub_key(url)?;
        self.registered_mut().insert(key.clone(), Arc::from(bytes));
        Ok(key)
    }

    /// Forgets what a request registered for `url`: whether there was something. A configured
    /// URL answers with its file again.
    pub fn remove(&self, url: &str) -> bool {
        stub_key(url).is_ok_and(|key| self.registered_mut().remove(&key).is_some())
    }

    /// Forgets what requests registered: how many URLs. The configured ones stay.
    pub fn clear(&self) -> usize {
        let mut registered = self.registered_mut();
        let count = registered.len();
        registered.clear();
        count
    }

    /// What `url` is answered with, `None` when it is not stubbed: the bytes, or why there are
    /// none (more than [`MAX_SOURCE_BYTES`], a file that cannot be read).
    pub fn get(&self, url: &str) -> Option<Outcome> {
        let key = stub_key(url).ok()?;
        if let Some(bytes) = self.registered().get(&key) {
            return Some(if bytes.len() > MAX_SOURCE_BYTES {
                Err(SourceError::TooLarge)
            } else {
                Ok(bytes.clone())
            });
        }
        let file = self.configured.get(&key)?;
        let read = std::fs::File::open(file)
            .and_then(|opened| read_at_most(opened, MAX_SOURCE_BYTES))
            .map_err(|error| {
                SourceError::Unreadable(format!(
                    "{url} is stubbed with {}, which cannot be read: {error}",
                    file.display()
                ))
            });
        Some(read.and_then(|bytes| bytes.map(Arc::from).ok_or(SourceError::TooLarge)))
    }

    /// Sorted by URL; a configured URL a request overrides is listed once, as the request's.
    pub fn list(&self) -> Vec<StubEntry> {
        let registered = self.registered();
        let mut entries: Vec<StubEntry> = registered
            .iter()
            .map(|(url, bytes)| StubEntry {
                url: url.clone(),
                size: Some(bytes.len() as u64),
                from: StubFrom::Request,
            })
            .chain(
                self.configured
                    .iter()
                    .filter(|(url, _)| !registered.contains_key(*url))
                    .map(|(url, file)| StubEntry {
                        url: url.clone(),
                        size: std::fs::metadata(file).ok().map(|meta| meta.len()),
                        from: StubFrom::File(file.clone()),
                    }),
            )
            .collect();
        entries.sort_by(|a, b| a.url.cmp(&b.url));
        entries
    }
}

/// Why lsf does not fetch from `ip` (`None` for a public address): what Shopify could not reach
/// either.
pub fn local_address(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(ip) => local_v4(ip),
        IpAddr::V6(ip) => local_v6(ip),
    }
}

fn local_v4(ip: Ipv4Addr) -> Option<&'static str> {
    let [a, b, ..] = ip.octets();
    Some(match (a, b) {
        _ if ip.is_unspecified() || a == 0 => "an unspecified address",
        _ if ip.is_loopback() => "a loopback address",
        (10, _) | (192, 168) => "a private address",
        (172, b) if (16..32).contains(&b) => "a private address",
        (100, b) if (64..128).contains(&b) => "a shared (carrier-grade NAT) address",
        _ if ip.is_link_local() => "a link-local address",
        (192, 0) if ip.octets()[2] == 0 => "an IETF protocol address",
        (198, 18 | 19) => "a benchmarking address",
        _ if ip.is_multicast() => "a multicast address",
        _ if ip.is_broadcast() || a >= 240 => "a reserved address",
        _ => return None,
    })
}

fn local_v6(ip: Ipv6Addr) -> Option<&'static str> {
    if let Some(v4) = embedded_v4(ip) {
        return local_v4(v4);
    }
    let first = ip.segments()[0];
    Some(match first {
        _ if ip.is_unspecified() => "an unspecified address",
        _ if ip.is_loopback() => "a loopback address",
        _ if first & 0xfe00 == 0xfc00 => "a unique local address",
        _ if first & 0xffc0 == 0xfe80 => "a link-local address",
        _ if first & 0xffc0 == 0xfec0 => "a site-local address",
        _ if first & 0xff00 == 0xff00 => "a multicast address",
        _ => return None,
    })
}

/// The IPv4 address an IPv6 one stands for: mapped (`::ffff:a.b.c.d`), compatible
/// (`::a.b.c.d`), NAT64 (`64:ff9b::a.b.c.d`), 6to4 (`2002:aabb:ccdd::`).
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = ip.segments();
    let tail = |high: u16, low: u16| {
        Ipv4Addr::new((high >> 8) as u8, high as u8, (low >> 8) as u8, low as u8)
    };
    match segments {
        [0, 0, 0, 0, 0, 0xffff, high, low] => Some(tail(high, low)),
        [0, 0, 0, 0, 0, 0, high, low] if !ip.is_loopback() && !ip.is_unspecified() => {
            Some(tail(high, low))
        }
        [0x64, 0xff9b, 0, 0, 0, 0, high, low] => Some(tail(high, low)),
        [0x2002, high, low, ..] => Some(tail(high, low)),
        _ => None,
    }
}

/// An address lsf does not fetch from, as the resolver refuses it.
#[derive(Debug)]
struct Refused {
    host: String,
    address: IpAddr,
    why: &'static str,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} resolves to {}, {}: Shopify cannot reach it, and lsf does not fetch it (start lsf \
             with --admin-fetch-local to fetch sources from your machine or network, or stub the \
             URL: --admin-source <url>=<file>, or PUT /__lsf/admin/sources?url=)",
            self.host, self.address, self.why
        )
    }
}

impl std::error::Error for Refused {}

/// The resolver of every connection of a source, redirects included: the addresses a host
/// resolves to, refused when one of them is one `refuse` names.
#[derive(Debug)]
struct PublicOnly {
    inner: ureq::unversioned::resolver::DefaultResolver,
    refuse: fn(IpAddr) -> Option<&'static str>,
}

impl ureq::unversioned::resolver::Resolver for PublicOnly {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let addresses = self.inner.resolve(uri, config, timeout)?;
        if let Some((address, why)) = addresses
            .iter()
            .find_map(|address| (self.refuse)(address.ip()).map(|why| (address.ip(), why)))
        {
            return Err(ureq::Error::Other(Box::new(Refused {
                host: uri.host().unwrap_or_default().to_string(),
                address,
                why,
            })));
        }
        Ok(addresses)
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
/// [`REQUEST_FETCH_BUDGET`], from the addresses `options` allow: what a request does before it
/// locks its shop. No lock is held. A URL `stubs` answers is not fetched, wherever it points.
pub fn prefetch(urls: &[String], options: FetchOptions, stubs: &Stubs) -> Fetched {
    let refuse: fn(IpAddr) -> Option<&'static str> = if options.allow_local {
        |_| None
    } else {
        local_address
    };
    let mut answered = HashMap::new();
    let mut fetching = Vec::new();
    for url in urls {
        match stubs.get(url) {
            Some(outcome) => {
                answered.insert(url.clone(), outcome);
            }
            None => fetching.push(url.clone()),
        }
    }
    let mut fetched = prefetch_with(&fetching, refuse);
    fetched.by_url.extend(answered);
    fetched
}

fn prefetch_with(urls: &[String], refuse: fn(IpAddr) -> Option<&'static str>) -> Fetched {
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
                    let result = fetch(url, deadline, refuse);
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

/// The bytes at an `http(s)` URL, within [`FETCH_TIMEOUT`] and before `deadline`, from an
/// address `refuse` does not name, on every hop.
fn fetch(url: &str, deadline: Instant, refuse: fn(IpAddr) -> Option<&'static str>) -> Outcome {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(SourceError::Unreadable(format!(
            "{url} was not fetched: the sources of the request took more than {} s",
            REQUEST_FETCH_BUDGET.as_secs()
        )));
    }
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT.min(left)))
        .timeout_connect(Some(Duration::from_secs(3).min(left)))
        .http_status_as_error(false)
        // A proxy of the environment would be what the resolver checks, not the source.
        .proxy(None)
        .build();
    let agent = ureq::Agent::with_parts(
        config,
        ureq::unversioned::transport::DefaultConnector::new(),
        PublicOnly {
            inner: Default::default(),
            refuse,
        },
    );
    let response = agent.get(url).call().map_err(|error| match error {
        ureq::Error::Other(inner) if inner.is::<Refused>() => {
            SourceError::Unreadable(format!("{url} is not fetched: {inner}"))
        }
        error => SourceError::Unreadable(format!("{url} could not be downloaded: {error}")),
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

    /// A server on `address` that counts its connections and answers each with `answer`: its
    /// port and the count.
    fn server(
        address: &str,
        answer: impl Fn(u16) -> String + Send + 'static,
    ) -> Option<(u16, Arc<AtomicUsize>)> {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind(address).ok()?;
        let port = listener.local_addr().ok()?.port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                counted.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    line.clear();
                }
                let _ = stream.write_all(answer(port).as_bytes());
            }
        });
        Some((port, hits))
    }

    fn ok(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn only_public_addresses_are_fetched_from() {
        for local in [
            "127.0.0.1",
            "127.255.0.1",
            "0.0.0.0",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.0.0.8",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "2002:c0a8:101::",
        ] {
            let ip: IpAddr = local.parse().unwrap();
            assert!(local_address(ip).is_some(), "{local} is not public");
        }
        for public in [
            "8.8.8.8",
            "93.184.216.34",
            "172.32.0.1",
            "100.128.0.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
            "2002:808:808::",
        ] {
            let ip: IpAddr = public.parse().unwrap();
            assert_eq!(local_address(ip), None, "{public} is public");
        }
    }

    /// Any http(s) URL was fetched, the loopback and private networks included, its body served
    /// back as a theme file on `/cdn/…`.
    #[test]
    fn a_local_source_is_refused_before_it_is_connected_to() {
        let (port, hits) = server("127.0.0.1:0", |_| ok("secret")).unwrap();
        let urls = [
            format!("http://127.0.0.1:{port}/admin"),
            format!("http://localhost:{port}/admin"),
            format!("http://LOCALHOST.:{port}/admin"),
            format!("http://[::ffff:127.0.0.1]:{port}/admin"),
        ];
        let fetched = prefetch(&urls, FetchOptions::default(), &Stubs::default());
        for url in &urls {
            let answer = fetched.get(url).unwrap();
            assert!(
                matches!(answer, Err(SourceError::Unreadable(why)) if why.contains("--admin-fetch-local")),
                "{url}: {answer:?}"
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing was connected to");
        // `file://` and the other schemes are not fetched at all.
        let shop = ShopState::seeded(
            &lsf_core::store::load::load(
                &lsf_core::store::load::DataSource::Demo,
                &Default::default(),
            )
            .0,
            None,
        );
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/x.zip",
            "gopher://127.0.0.1/",
        ] {
            assert!(
                matches!(bytes_of(&shop, Some(&fetched), url), Err(SourceError::Unreadable(why)) if why.contains("neither")),
                "{url}"
            );
        }
        // With `--admin-fetch-local`, it is fetched.
        let allowed = prefetch(
            &urls[..1],
            FetchOptions { allow_local: true },
            &Stubs::default(),
        );
        assert_eq!(
            allowed.get(&urls[0]).unwrap().as_deref().ok(),
            Some(&b"secret"[..])
        );
    }

    /// A public source that redirects to a local one: the address is checked on every hop.
    #[test]
    fn every_redirect_hop_is_checked() {
        let Some((target, hits)) = server("[::1]:0", |_| ok("secret")) else {
            eprintln!("no IPv6 loopback here: the redirect hop is not tested");
            return;
        };
        let (port, _) = server("127.0.0.1:0", move |_| {
            format!(
                "HTTP/1.1 302 Found\r\nlocation: http://[::1]:{target}/admin\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            )
        })
        .unwrap();
        // The first hop's address passes, the second's does not.
        let only_v6_loopback =
            |ip: IpAddr| (ip == IpAddr::V6(Ipv6Addr::LOCALHOST)).then_some("a loopback address");
        let url = format!("http://127.0.0.1:{port}/theme.zip");
        let fetched = prefetch_with(std::slice::from_ref(&url), only_v6_loopback);
        let answer = fetched.get(&url).unwrap();
        assert!(
            matches!(answer, Err(SourceError::Unreadable(why)) if why.contains("::1")),
            "{answer:?}"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "the redirect was not followed"
        );
    }

    #[test]
    fn a_stub_answers_for_its_url_as_a_client_writes_it() {
        assert_eq!(
            stub_key(" HTTPS://CDN.Example.com/Themes/a.zip?v=1#top ").as_deref(),
            Ok("https://cdn.example.com/Themes/a.zip?v=1")
        );
        assert_eq!(
            stub_key("http://example.com").as_deref(),
            Ok("http://example.com/")
        );
        assert_eq!(
            stub_key("http://example.com?x=1").as_deref(),
            Ok("http://example.com/?x=1")
        );
        for bad in [
            "file:///etc/passwd",
            "ftp://example.com/a.zip",
            "https:///a.zip",
            "cdn.example.com/a.zip",
            "http://127.0.0.1:9292/__lsf/admin/uploads/shop/tmp/1/a.zip",
        ] {
            assert!(stub_key(bad).is_err(), "{bad}");
        }
    }

    /// A stubbed URL is answered by lsf, never connected to: on the loopback without
    /// `--admin-fetch-local`, a request's bytes over a configured file, the file again once they
    /// are deleted.
    #[test]
    fn a_stubbed_url_is_never_fetched() {
        let (port, hits) = server("127.0.0.1:0", |_| ok("from the network")).unwrap();
        let url = format!("http://127.0.0.1:{port}/theme.zip");
        let dir = std::env::temp_dir().join(format!("lsf-stubs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("theme.zip");
        std::fs::write(&file, b"from the file").unwrap();
        let stubs = Stubs::configured([(url.clone(), file.clone())]);
        let bytes_of = |stubs: &Stubs| {
            prefetch(std::slice::from_ref(&url), FetchOptions::default(), stubs)
                .get(&url)
                .cloned()
                .unwrap()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        };
        assert_eq!(bytes_of(&stubs).as_deref(), Ok("from the file"));
        // An edit of the file is seen at once.
        std::fs::write(&file, b"edited").unwrap();
        assert_eq!(bytes_of(&stubs).as_deref(), Ok("edited"));
        // A request's bytes override it, as the URL is written or not.
        let key = stubs
            .register(
                &url.replace("http://", "HTTP://"),
                b"from a request".to_vec(),
            )
            .unwrap();
        assert_eq!(key, url);
        assert_eq!(bytes_of(&stubs).as_deref(), Ok("from a request"));
        assert_eq!(
            stubs.list(),
            [StubEntry {
                url: url.clone(),
                size: Some(14),
                from: StubFrom::Request
            }]
        );
        assert!(stubs.remove(&url));
        assert!(!stubs.remove(&url));
        assert_eq!(bytes_of(&stubs).as_deref(), Ok("edited"));
        assert_eq!(stubs.list()[0].from, StubFrom::File(file.clone()));
        stubs
            .register("https://example.com/a.csv", Vec::new())
            .unwrap();
        assert_eq!(stubs.clear(), 1);
        assert_eq!(stubs.list().len(), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing was connected to");
        std::fs::remove_file(&file).unwrap();
        assert!(
            matches!(bytes_of(&stubs), Err(SourceError::Unreadable(why)) if why.contains("cannot be read")),
        );
        assert_eq!(stubs.list()[0].size, None);
        stubs.register(&url, vec![0; MAX_SOURCE_BYTES + 1]).unwrap();
        assert_eq!(bytes_of(&stubs), Err(SourceError::TooLarge));
        assert!(stubs.get("https://example.com/other.zip").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
