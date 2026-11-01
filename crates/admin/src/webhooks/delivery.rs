//! Sending webhooks, and the log of what was sent.
//!
//! Deliveries are sent by worker threads of their own, never by the thread of the request that
//! caused them: the handler of the backend may call the Admin API back while it answers. A `2xx`
//! within the timeout (5 seconds, as Shopify) is a success. Anything else (another status, a
//! redirect, a timeout, a connection that fails) is retried after each delay of
//! [`DeliveryOptions::retry_delays`], then marked failed: Shopify retries 8 times over 4 hours,
//! which a test cannot wait for.
//!
//! Deliveries go out one at a time by default, in the order they were made: Shopify promises no
//! order, but a log read in the order things happened is easier to follow. A delivery waiting for
//! its retry does not hold the others back.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::io::Read;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::Serialize;

use super::sign::USER_AGENT;
use super::subscriptions::Subscription;

/// How deliveries are sent.
#[derive(Clone, Debug)]
pub struct DeliveryOptions {
    /// How long a delivery may take, from connecting to the end of the answer: 5 seconds, as
    /// Shopify.
    pub timeout: Duration,
    /// How long connecting may take: 1 second, as Shopify.
    pub connect_timeout: Duration,
    /// The delay before each retry: as many retries as delays. 1, 5 and 15 seconds by default;
    /// none in tests.
    pub retry_delays: Vec<Duration>,
    /// How many deliveries are sent at once. 1 by default: in order.
    pub workers: usize,
    /// How many deliveries the log keeps; the oldest are forgotten first.
    pub keep: usize,
    /// What the reason of a delivery the backend refuses (`401`, `403`) adds: the secret lsf
    /// signed it with, and where that secret came from ([`crate::auth::SecretSource::refusal`]).
    pub refusal_hint: Option<String>,
}

impl Default for DeliveryOptions {
    fn default() -> DeliveryOptions {
        DeliveryOptions {
            timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(1),
            retry_delays: vec![
                Duration::from_secs(1),
                Duration::from_secs(5),
                Duration::from_secs(15),
            ],
            workers: 1,
            keep: 1000,
            refusal_hint: None,
        }
    }
}

/// Where a delivery is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Waiting to be sent, being sent, or waiting for a retry.
    Pending,
    /// Answered with a `2xx`.
    Delivered,
    /// Every attempt failed.
    Failed,
    /// Never sent, for the reason the delivery gives: a topic whose payload lsf does not build,
    /// a filter that does not match, an address lsf cannot deliver to.
    NotSent,
}

impl DeliveryState {
    pub fn parse(text: &str) -> Option<DeliveryState> {
        match text {
            "pending" => Some(DeliveryState::Pending),
            "delivered" => Some(DeliveryState::Delivered),
            "failed" => Some(DeliveryState::Failed),
            "not_sent" | "not-sent" => Some(DeliveryState::NotSent),
            _ => None,
        }
    }
}

/// The subscription a delivery is for.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionRef {
    pub id: String,
    pub name: Option<String>,
    /// The `uri` as the subscription gives it.
    pub uri: String,
}

impl SubscriptionRef {
    pub fn of(subscription: &Subscription) -> SubscriptionRef {
        SubscriptionRef {
            id: subscription.id.clone(),
            name: subscription.name.clone(),
            uri: subscription.uri.clone(),
        }
    }
}

/// The request of a delivery, as sent.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Request {
    /// As Shopify writes their names.
    pub headers: IndexMap<String, String>,
    /// The raw body, the bytes the signature is of.
    pub body: String,
}

/// One try at sending a delivery.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    /// The status of the answer, when there was one.
    pub status: Option<u16>,
    /// The start of the body of the answer.
    pub response: Option<String>,
    /// What went wrong: a status out of `2xx`, a timeout, a connection refused.
    pub error: Option<String>,
}

/// A webhook delivery: one event, one subscription (or one address).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    /// The position of the delivery in the log, from 1.
    pub id: u64,
    /// `X-Shopify-Webhook-Id`.
    pub webhook_id: String,
    /// `X-Shopify-Event-Id`.
    pub event_id: String,
    pub topic: String,
    pub shop: String,
    /// The GID of the record the event is about (the payload's `admin_graphql_api_id`), when
    /// there is one: what `?resource=` matches.
    pub resource: Option<String>,
    /// `None` for a delivery to an address given explicitly (`lsf app webhook trigger
    /// --address`).
    pub subscription: Option<SubscriptionRef>,
    /// Where it is sent; `None` when the subscription's `uri` is not an HTTP address.
    pub url: Option<String>,
    pub api_version: String,
    /// `X-Shopify-Triggered-At`.
    pub triggered_at: DateTime<Utc>,
    /// The request, when there is a body to send.
    pub request: Option<Request>,
    pub attempts: Vec<Attempt>,
    pub state: DeliveryState,
    /// Why it was not sent, or why it failed.
    pub reason: Option<String>,
    /// What lsf did differently from Shopify for it: a filter it did not evaluate, the API
    /// version of the payload.
    pub notes: Vec<String>,
}

/// A delivery about to be logged: everything but its id and its outcome.
#[derive(Clone, Debug)]
pub struct NewDelivery {
    pub webhook_id: String,
    pub event_id: String,
    pub topic: String,
    pub shop: String,
    pub resource: Option<String>,
    pub subscription: Option<SubscriptionRef>,
    pub url: Option<String>,
    pub api_version: String,
    pub triggered_at: DateTime<Utc>,
    pub request: Option<Request>,
    pub notes: Vec<String>,
}

/// Which deliveries a listing or a wait is about. Every field left `None` matches all.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeliveryFilter {
    pub topic: Option<String>,
    /// The shop's domain, written in full (`test-1.myshopify.com`).
    pub shop: Option<String>,
    pub state: Option<DeliveryState>,
    /// Only the deliveries after this one, by id: those a test caused since it read the log.
    pub since: Option<u64>,
    /// Only those about this record, by GID ([`Delivery::resource`]).
    pub resource: Option<String>,
    /// Only these deliveries, by id.
    pub ids: Option<Vec<u64>>,
}

impl DeliveryFilter {
    pub fn matches(&self, delivery: &Delivery) -> bool {
        self.topic
            .as_ref()
            .is_none_or(|topic| *topic == delivery.topic)
            && self.shop.as_ref().is_none_or(|shop| *shop == delivery.shop)
            && self.state.is_none_or(|state| state == delivery.state)
            && self.since.is_none_or(|since| delivery.id > since)
            && self
                .resource
                .as_ref()
                .is_none_or(|resource| delivery.resource.as_ref() == Some(resource))
            && self
                .ids
                .as_ref()
                .is_none_or(|ids| ids.contains(&delivery.id))
    }

    /// Whether a delivery that matches counts as done for a wait: one that is not pending, or
    /// any when the filter asks for pending ones.
    fn done(&self, delivery: &Delivery) -> bool {
        self.matches(delivery)
            && (delivery.state != DeliveryState::Pending
                || self.state == Some(DeliveryState::Pending))
    }
}

/// What a wait found.
#[derive(Clone, Debug, PartialEq)]
pub struct Waited {
    /// The deliveries that match, done or not, in the order of the log.
    pub deliveries: Vec<Delivery>,
    /// Whether as many as were waited for were done before the timeout.
    pub complete: bool,
    /// The id of the newest delivery of the log when the wait ended ([`Deliveries::listing`]).
    pub last: u64,
}

/// The deliveries that match, and the id of the newest delivery of the log, read together.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    pub deliveries: Vec<Delivery>,
    /// The id of the newest delivery the log has made, whatever the filter (0 before the first):
    /// what a test passes as `since` to see only the deliveries that come after. Ids are never
    /// given twice, a `DELETE` or a full log notwithstanding.
    pub last: u64,
}

struct Log {
    entries: VecDeque<Delivery>,
    /// The deliveries due to be sent, soonest first: when, and their id.
    queue: BinaryHeap<Reverse<(Instant, u64)>>,
    last_id: u64,
    shutdown: bool,
}

struct Shared {
    log: Mutex<Log>,
    /// Wakes the workers: a delivery to send, or the end.
    work: Condvar,
    /// Wakes the waiters: a delivery changed.
    changed: Condvar,
    options: DeliveryOptions,
    /// Prints a line for each delivery the backend refuses ([`Deliveries::print_refusals`]).
    refusals: OnceLock<RefusalLog>,
}

/// What prints the line of a delivery the backend refused.
pub type RefusalLog = Arc<dyn Fn(&str) + Send + Sync>;

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Log> {
        // A worker that panicked leaves the log as it was: still worth reading.
        self.log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The log of deliveries, and the workers that send them. Dropping it stops the workers (one in
/// the middle of a request finishes it first).
pub struct Deliveries {
    shared: Arc<Shared>,
}

impl Deliveries {
    /// Starts the workers.
    pub fn new(options: DeliveryOptions) -> Deliveries {
        let workers = options.workers.max(1);
        let shared = Arc::new(Shared {
            log: Mutex::new(Log {
                entries: VecDeque::new(),
                queue: BinaryHeap::new(),
                last_id: 0,
                shutdown: false,
            }),
            work: Condvar::new(),
            changed: Condvar::new(),
            options,
            refusals: OnceLock::new(),
        });
        let agent = agent(&shared.options);
        for index in 0..workers {
            let shared = Arc::clone(&shared);
            let agent = agent.clone();
            // A worker that cannot start leaves the deliveries pending: the log says so.
            let _ = std::thread::Builder::new()
                .name(format!("lsf-webhooks-{index}"))
                .spawn(move || work(&shared, &agent));
        }
        Deliveries { shared }
    }

    pub fn options(&self) -> &DeliveryOptions {
        &self.shared.options
    }

    /// Prints with `log`, once per delivery, that the backend refused it (`401`, `403`), at its
    /// first refusal: `products/create → http://localhost:3000/webhooks answered 401: …`, with
    /// [`DeliveryOptions::refusal_hint`]. Set once; a second call changes nothing.
    pub fn print_refusals(&self, log: RefusalLog) {
        let _ = self.shared.refusals.set(log);
    }

    /// Logs a delivery and queues it to be sent at once. Its id.
    pub fn send(&self, delivery: NewDelivery) -> u64 {
        let mut log = self.shared.lock();
        let id = push(
            &mut log,
            delivery,
            DeliveryState::Pending,
            None,
            self.shared.options.keep,
        );
        log.queue.push(Reverse((Instant::now(), id)));
        drop(log);
        self.shared.work.notify_one();
        self.shared.changed.notify_all();
        id
    }

    /// Logs a delivery that is not sent, and why. Its id.
    pub fn not_sent(&self, delivery: NewDelivery, reason: String) -> u64 {
        let mut log = self.shared.lock();
        let id = push(
            &mut log,
            delivery,
            DeliveryState::NotSent,
            Some(reason),
            self.shared.options.keep,
        );
        drop(log);
        self.shared.changed.notify_all();
        id
    }

    /// The deliveries that match, in the order of the log.
    pub fn list(&self, filter: &DeliveryFilter) -> Vec<Delivery> {
        self.listing(filter).deliveries
    }

    /// The deliveries that match, in the order of the log, and the id of its newest delivery.
    pub fn listing(&self, filter: &DeliveryFilter) -> Listing {
        let log = self.shared.lock();
        Listing {
            deliveries: log
                .entries
                .iter()
                .filter(|delivery| filter.matches(delivery))
                .cloned()
                .collect(),
            last: log.last_id,
        }
    }

    /// Forgets every delivery, those still pending included (they are not sent). How many there
    /// were.
    pub fn clear(&self) -> usize {
        let mut log = self.shared.lock();
        let count = log.entries.len();
        log.entries.clear();
        log.queue.clear();
        drop(log);
        self.shared.changed.notify_all();
        count
    }

    /// Waits until `count` deliveries that match are done (delivered, failed or not sent), or
    /// until `timeout`.
    pub fn wait(&self, filter: &DeliveryFilter, count: usize, timeout: Duration) -> Waited {
        let deadline = Instant::now() + timeout;
        let mut log = self.shared.lock();
        loop {
            let done = log
                .entries
                .iter()
                .filter(|delivery| filter.done(delivery))
                .count();
            let now = Instant::now();
            if done >= count || now >= deadline || log.shutdown {
                let deliveries = log
                    .entries
                    .iter()
                    .filter(|delivery| filter.matches(delivery))
                    .cloned()
                    .collect();
                return Waited {
                    deliveries,
                    complete: done >= count,
                    last: log.last_id,
                };
            }
            log = self
                .shared
                .changed
                .wait_timeout(log, deadline - now)
                .map_or_else(|poisoned| poisoned.into_inner().0, |(log, _)| log);
        }
    }
}

impl Drop for Deliveries {
    fn drop(&mut self) {
        self.shared.lock().shutdown = true;
        self.shared.work.notify_all();
        self.shared.changed.notify_all();
    }
}

fn push(
    log: &mut Log,
    delivery: NewDelivery,
    state: DeliveryState,
    reason: Option<String>,
    keep: usize,
) -> u64 {
    log.last_id += 1;
    let id = log.last_id;
    log.entries.push_back(Delivery {
        id,
        webhook_id: delivery.webhook_id,
        event_id: delivery.event_id,
        topic: delivery.topic,
        shop: delivery.shop,
        resource: delivery.resource,
        subscription: delivery.subscription,
        url: delivery.url,
        api_version: delivery.api_version,
        triggered_at: delivery.triggered_at,
        request: delivery.request,
        attempts: Vec::new(),
        state,
        reason,
        notes: delivery.notes,
    });
    while log.entries.len() > keep.max(1) {
        log.entries.pop_front();
    }
    id
}

/// The HTTP client of the workers: no redirect is followed (Shopify takes a `3xx` for a failure),
/// a status out of `2xx` is an answer, not an error, and a name under `localhost` connects to
/// the loopback ([`LocalhostNames`]).
fn agent(options: &DeliveryOptions) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(options.timeout))
        .timeout_connect(Some(options.connect_timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .user_agent(USER_AGENT)
        .build();
    ureq::Agent::with_parts(
        config,
        ureq::unversioned::transport::DefaultConnector::default(),
        LocalhostNames::default(),
    )
}

/// Resolves a name under `localhost` (`app.localhost`) to the loopback, `127.0.0.1` then `::1`,
/// whatever the system resolver says of it: RFC 6761 reserves those names for the loopback, and
/// lsf counts them as this machine ([`super::subscriptions::is_loopback_host`]), but a system
/// resolver need not know them. The request keeps its `Host`. Any other name, `localhost`
/// itself included, is the system resolver's.
#[derive(Debug, Default)]
struct LocalhostNames(ureq::unversioned::resolver::DefaultResolver);

impl ureq::unversioned::resolver::Resolver for LocalhostNames {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let under_localhost = uri.host().is_some_and(|host| {
            !host.eq_ignore_ascii_case("localhost") && super::subscriptions::is_localhost_name(host)
        });
        if !under_localhost {
            return self.0.resolve(uri, config, timeout);
        }
        let port = uri.port_u16().unwrap_or(match uri.scheme_str() {
            Some(scheme) if scheme.eq_ignore_ascii_case("https") => 443,
            _ => 80,
        });
        let loopback = [
            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
        ];
        let mut addresses = self.empty();
        for address in config.ip_family().keep_wanted(loopback.into_iter()) {
            addresses.push(address);
        }
        if addresses.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(addresses)
    }
}

/// A worker: sends the deliveries that are due, until the log is dropped.
fn work(shared: &Shared, agent: &ureq::Agent) {
    loop {
        let job = {
            let mut log = shared.lock();
            loop {
                if log.shutdown {
                    return;
                }
                let now = Instant::now();
                match log.queue.peek().copied() {
                    Some(Reverse((due, id))) if due <= now => {
                        log.queue.pop();
                        // A delivery cleared from the log is not sent.
                        let job = log
                            .entries
                            .iter()
                            .find(|delivery| delivery.id == id)
                            .and_then(|delivery| {
                                Some((id, delivery.url.clone()?, delivery.request.clone()?))
                            });
                        if let Some(job) = job {
                            break job;
                        }
                    }
                    Some(Reverse((due, _))) => {
                        log = shared
                            .work
                            .wait_timeout(log, due - now)
                            .map_or_else(|poisoned| poisoned.into_inner().0, |(log, _)| log);
                    }
                    None => {
                        log = shared
                            .work
                            .wait(log)
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                    }
                }
            }
        };
        let (id, url, request) = job;
        let attempt = attempt(agent, &url, &request, &shared.options);
        let mut log = shared.lock();
        let retries = shared.options.retry_delays.len();
        let Some(delivery) = log.entries.iter_mut().find(|delivery| delivery.id == id) else {
            continue;
        };
        let succeeded = attempt.error.is_none();
        let error = attempt.error.clone();
        // A backend that refuses the signature answers this way: the reason says which secret
        // lsf signed with, and the server says it at the first refusal.
        let hint = shared
            .options
            .refusal_hint
            .as_deref()
            .filter(|_| is_refusal(attempt.status));
        let first_refusal =
            hint.is_some() && !delivery.attempts.iter().any(|done| is_refusal(done.status));
        delivery.attempts.push(attempt);
        let tried = delivery.attempts.len();
        let with_hint = |reason: String| match hint {
            Some(hint) => format!("{reason}: {hint}"),
            None => reason,
        };
        let retry = if succeeded {
            delivery.state = DeliveryState::Delivered;
            delivery.reason = None;
            None
        } else if tried <= retries {
            delivery.reason = error.map(with_hint);
            Some(shared.options.retry_delays[tried - 1])
        } else {
            delivery.state = DeliveryState::Failed;
            delivery.reason = error.map(|error| {
                with_hint(format!(
                    "{error} ({tried} attempt{})",
                    if tried == 1 { "" } else { "s" }
                ))
            });
            None
        };
        let refused = first_refusal
            .then(|| delivery.attempts.last()?.error.clone())
            .flatten()
            .map(|error| format!("{} → {url} {}", delivery.topic, with_hint(error)));
        if let Some(delay) = retry {
            log.queue.push(Reverse((Instant::now() + delay, id)));
            shared.work.notify_one();
        }
        drop(log);
        shared.changed.notify_all();
        if let (Some(line), Some(print)) = (refused, shared.refusals.get()) {
            print(&line);
        }
    }
}

/// Whether the backend refused the delivery itself (`401`, `403`): what a backend whose secret
/// is not lsf's answers.
fn is_refusal(status: Option<u16>) -> bool {
    matches!(status, Some(401 | 403))
}

/// How much of the answer the log keeps.
const RESPONSE_START: usize = 1024;

/// Sends a delivery once.
fn attempt(
    agent: &ureq::Agent,
    url: &str,
    request: &Request,
    options: &DeliveryOptions,
) -> Attempt {
    let started_at = Utc::now();
    let start = Instant::now();
    let outcome = post(agent, url, request);
    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Ok((status, response)) => Attempt {
            started_at,
            duration_ms,
            status: Some(status),
            response: Some(response),
            error: (!(200..300).contains(&status)).then(|| {
                if (300..400).contains(&status) {
                    format!("answered {status}: a redirect is a failure, as on Shopify")
                } else {
                    format!("answered {status}")
                }
            }),
        },
        Err(error) => Attempt {
            started_at,
            duration_ms,
            status: None,
            response: None,
            error: Some(describe(&error, url, options)),
        },
    }
}

fn post(agent: &ureq::Agent, url: &str, request: &Request) -> Result<(u16, String), ureq::Error> {
    let mut builder = ureq::http::Request::builder().method("POST").uri(url);
    for (name, value) in &request.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let http_request = builder
        .body(request.body.clone())
        .map_err(ureq::Error::Http)?;
    let response = agent.run(http_request)?;
    let status = response.status().as_u16();
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    // The start of the answer is kept; the rest is read, up to a limit, so that the connection
    // can serve the next delivery.
    let mut reader = body.as_reader();
    reader
        .by_ref()
        .take(RESPONSE_START as u64)
        .read_to_end(&mut bytes)?;
    let _ = std::io::copy(&mut reader.take(1 << 20), &mut std::io::sink());
    Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
}

/// What went wrong, in words a developer acts on.
fn describe(error: &ureq::Error, url: &str, options: &DeliveryOptions) -> String {
    use ureq::Error;
    let timed_out = |kind: std::io::ErrorKind| {
        matches!(
            kind,
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        )
    };
    let no_answer = || {
        format!(
            "no answer within {} s: Shopify takes that for a failure",
            options.timeout.as_secs_f64()
        )
    };
    match error {
        Error::Timeout(ureq::Timeout::Connect) => format!(
            "no connection within {} s: Shopify takes that for a failure (is the backend of the app listening on {url}?)",
            options.connect_timeout.as_secs_f64()
        ),
        Error::Timeout(_) => no_answer(),
        Error::Io(io) if timed_out(io.kind()) => no_answer(),
        Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            format!("connection refused: is the backend of the app listening on {url}?")
        }
        Error::ConnectionFailed => {
            format!("connection failed: is the backend of the app listening on {url}?")
        }
        Error::HostNotFound => format!("the host of {url} was not found"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_under_localhost_resolves_to_the_loopback() {
        use ureq::unversioned::resolver::Resolver;
        use ureq::unversioned::transport::NextTimeout;

        let resolve = |url: &str| {
            let timeout = NextTimeout {
                after: Duration::from_secs(1).into(),
                reason: ureq::Timeout::Resolve,
                per_read: None,
                per_write: None,
            };
            LocalhostNames::default()
                .resolve(
                    &url.parse().unwrap(),
                    &ureq::config::Config::default(),
                    timeout,
                )
                .map(|addresses| {
                    addresses
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                })
        };
        // Never asked of the system resolver, which need not know the name.
        assert_eq!(
            resolve("http://no-such-app.localhost:3051/webhooks").unwrap(),
            ["127.0.0.1:3051", "[::1]:3051"]
        );
        assert_eq!(
            resolve("HTTPS://Shop.App.LOCALHOST/x").unwrap(),
            ["127.0.0.1:443", "[::1]:443"]
        );
        // Any other name is the system resolver's.
        assert_eq!(
            resolve("http://127.0.0.1:3051/x").unwrap(),
            ["127.0.0.1:3051"]
        );
    }

    #[test]
    fn a_timeout_says_which_one_fired() {
        let options = DeliveryOptions::default();
        let url = "http://localhost:3000/webhooks";
        let connect = describe(&ureq::Error::Timeout(ureq::Timeout::Connect), url, &options);
        assert!(connect.starts_with("no connection within 1 s"), "{connect}");
        let answer = describe(&ureq::Error::Timeout(ureq::Timeout::Global), url, &options);
        assert!(answer.starts_with("no answer within 5 s"), "{answer}");
    }
}
