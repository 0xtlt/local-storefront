//! `/__lsf/webhooks`: the deliveries, for tests.
//!
//! - `GET /__lsf/webhooks`: the deliveries, oldest first. `?topic=orders/create`, `?shop=`,
//!   `?state=delivered|failed|pending|not_sent` narrow them; `?wait=<n>` waits until n of them are
//!   done (not pending), `?timeout=<ms>` at most (10 seconds by default), and answers `408` when
//!   they were not.
//! - `DELETE /__lsf/webhooks`: forgets them.
//! - `POST /__lsf/webhooks/trigger`: sends one, as `lsf app webhook trigger` does:
//!   `{"topic", "id"?, "shop"?, "address"?, "payload"?, "wait"?}`.
//!
//! Nothing here touches a socket: `lsf` routes the request to [`Webhooks::handle`].

use std::time::Duration;

use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use super::delivery::{DeliveryFilter, DeliveryState};
use super::{Trigger, Webhooks, topics};
use crate::api::{HttpRequest, HttpResponse};

/// How long a wait lasts when the request does not say.
const DEFAULT_WAIT: Duration = Duration::from_secs(10);

/// The longest wait a request may ask for.
const LONGEST_WAIT: Duration = Duration::from_secs(120);

/// The body of `POST /__lsf/webhooks/trigger`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TriggerRequest {
    /// `orders/create` or `ORDERS_CREATE`.
    pub topic: String,
    /// The record to build the payload from, by GID. The api layer picks one when there is none.
    pub id: Option<String>,
    /// The shop, by domain: the server's shop when there is none.
    pub shop: Option<String>,
    /// Where to send it, rather than to the subscriptions of the topic.
    pub address: Option<String>,
    /// The body to send, as is: then nothing is built.
    pub payload: Option<Value>,
    /// Answer once the deliveries are done, rather than at once.
    #[serde(default)]
    pub wait: bool,
}

/// What the api layer gives [`Webhooks::handle`] to build the payload of a trigger: the shop it is
/// for (the default one when the trigger names none, its domain written in full), and the payload
/// built from its state, or the trigger's own `payload` as is.
pub type BuildTrigger<'a> = dyn Fn(&TriggerRequest) -> Result<(String, Value), TriggerError> + 'a;

/// Why the api layer could not build the payload of a trigger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerError {
    /// No record to build it from (`404`): the id names none, or the shop has none of the type.
    NotFound(String),
    /// lsf does not build the payload of the topic (`422`).
    NotBuilt(String),
}

impl Webhooks {
    /// The answer to a request under `/__lsf/webhooks`, `None` for another path. `build` is the
    /// api layer's: the shop and the payload of a trigger, built from the state for the record of
    /// `request.id` (or one it picks for the topic).
    pub fn handle(&self, request: &HttpRequest, build: &BuildTrigger) -> Option<HttpResponse> {
        let path = request.path.trim_end_matches('/');
        Some(match (request.method.as_str(), path) {
            ("GET", "/__lsf/webhooks") => self.list_response(request),
            ("DELETE", "/__lsf/webhooks") => {
                let cleared = self.deliveries().clear();
                HttpResponse::json(200, &json!({"ok": true, "deliveries": cleared}))
            }
            ("POST", "/__lsf/webhooks/trigger") => self.trigger_response(request, build),
            (_, "/__lsf/webhooks" | "/__lsf/webhooks/trigger") => HttpResponse::json(
                405,
                &json!({"error": format!("{} is not allowed here", request.method)}),
            ),
            _ => return None,
        })
    }

    fn list_response(&self, request: &HttpRequest) -> HttpResponse {
        let state = match request.query_param("state") {
            Some(state) => match DeliveryState::parse(state) {
                Some(state) => Some(state),
                None => {
                    return HttpResponse::json(
                        422,
                        &json!({"error": format!(
                            "`{state}` is not a state: pending, delivered, failed or not_sent"
                        )}),
                    );
                }
            },
            None => None,
        };
        let topic = match request.query_param("topic") {
            Some(topic) => match topics::written_form(topic) {
                Some(topic) => Some(topic.to_string()),
                None => return unknown_topic(topic),
            },
            None => None,
        };
        let filter = DeliveryFilter {
            topic,
            shop: request.query_param("shop").map(str::to_string),
            state,
            ids: None,
        };
        let Some(wait) = request.query_param("wait") else {
            let deliveries = self.deliveries().list(&filter);
            return HttpResponse::json(
                200,
                &json!({"count": deliveries.len(), "deliveries": deliveries}),
            );
        };
        let Ok(count) = wait.parse::<usize>() else {
            return HttpResponse::json(
                422,
                &json!({"error": format!("`wait` is a number of deliveries, got `{wait}`")}),
            );
        };
        let timeout = match timeout_of(request) {
            Ok(timeout) => timeout,
            Err(response) => return response,
        };
        let waited = self.deliveries().wait(&filter, count, timeout);
        HttpResponse::json(
            if waited.complete { 200 } else { 408 },
            &json!({
                "count": waited.deliveries.len(),
                "deliveries": waited.deliveries,
                "waited": {"for": count, "complete": waited.complete, "timeoutMs": timeout.as_millis() as u64},
            }),
        )
    }

    fn trigger_response(&self, request: &HttpRequest, build: &BuildTrigger) -> HttpResponse {
        let trigger: TriggerRequest = match serde_json::from_slice(&request.body) {
            Ok(trigger) => trigger,
            Err(error) => {
                return HttpResponse::json(
                    422,
                    &json!({"error": format!(
                        "the body is {{\"topic\", \"id\"?, \"shop\"?, \"address\"?, \"payload\"?, \"wait\"?}}: {error}"
                    )}),
                );
            }
        };
        let Some(topic) = topics::written_form(&trigger.topic) else {
            return unknown_topic(&trigger.topic);
        };
        let normalized = TriggerRequest {
            topic: topic.to_string(),
            ..trigger.clone()
        };
        let (shop, payload) = match build(&normalized) {
            Ok(built) => built,
            Err(TriggerError::NotFound(message)) => {
                return HttpResponse::json(404, &json!({"error": message}));
            }
            Err(TriggerError::NotBuilt(message)) => {
                return HttpResponse::json(422, &json!({"error": message}));
            }
        };
        let ids = self.trigger(&Trigger {
            topic,
            shop: &shop,
            payload: &payload,
            address: trigger.address.as_deref(),
            at: Utc::now(),
        });
        if ids.is_empty() {
            return HttpResponse::json(
                404,
                &json!({"error": format!(
                    "no subscription to `{topic}` for {shop}, in shopify.app.toml or through the \
                     Admin API: subscribe to it, or give an `address`"
                )}),
            );
        }
        let filter = DeliveryFilter {
            ids: Some(ids.clone()),
            ..Default::default()
        };
        let deliveries = if trigger.wait {
            let timeout = match timeout_of(request) {
                Ok(timeout) => timeout,
                Err(response) => return response,
            };
            self.deliveries()
                .wait(&filter, ids.len(), timeout)
                .deliveries
        } else {
            self.deliveries().list(&filter)
        };
        HttpResponse::json(
            200,
            &json!({"count": deliveries.len(), "deliveries": deliveries}),
        )
    }
}

/// `?timeout=<ms>`, 10 seconds without it.
fn timeout_of(request: &HttpRequest) -> Result<Duration, HttpResponse> {
    match request.query_param("timeout") {
        None => Ok(DEFAULT_WAIT),
        Some(text) => match text.parse::<u64>() {
            Ok(ms) => Ok(Duration::from_millis(ms).min(LONGEST_WAIT)),
            Err(_) => Err(HttpResponse::json(
                422,
                &json!({"error": format!("`timeout` is in milliseconds, got `{text}`")}),
            )),
        },
    }
}

fn unknown_topic(topic: &str) -> HttpResponse {
    let hint = topics::suggestion(topic)
        .map(|suggestion| format!(" (did you mean `{suggestion}`?)"))
        .unwrap_or_default();
    HttpResponse::json(
        422,
        &json!({"error": format!("`{topic}` is not a webhook topic of API 2026-10{hint}")}),
    )
}
