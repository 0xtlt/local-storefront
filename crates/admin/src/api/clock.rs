//! `GET` and `POST /__lsf/admin/clock`: the clock of a shop, and moving it on.
//!
//! What waits on time in Shopify waits on the shop's clock in lsf: a checkout is abandoned ten
//! minutes after its last change, a data erasure is processed ten days after it was asked for,
//! the admin deletes abandoned checkouts after three months. A store data with a `now` keeps
//! the clock still, so that a test reads the same dates every run; this endpoint moves it:
//!
//! - `{"advance": "11m"}` (`s`, `m`, `h`, `d`, or a number of seconds) or `{"at": "<RFC 3339>"}`,
//!   and `"shop"` (a name or a domain; the default shop otherwise). The clock is then frozen
//!   at that instant, whether it was the store data's or the system's.
//! - Then what the time passed brings is done, with its webhooks: the data erasures due are
//!   processed (`customers/redact` sent to the app's compliance subscription, as Shopify does
//!   when it erases), and the abandoned checkouts past three months deleted
//!   (`checkouts/delete`).
//!
//! The answer is `{"shop", "now", "erased": [<customer GIDs>], "deletedCheckouts": <n>}`. A reset
//! (`DELETE /__lsf/admin`) puts the clock back as the store data has it.

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::json;

use super::{AdminApi, HttpRequest, HttpResponse};
use crate::state::Clock;

#[derive(Debug, Default, Deserialize)]
struct ClockBody {
    shop: Option<String>,
    advance: Option<serde_json::Value>,
    at: Option<String>,
}

/// A duration as the body writes it: `"11m"`, `"10d"`, `"2h"`, `"30s"`, or seconds.
fn duration(value: &serde_json::Value) -> Option<Duration> {
    if let Some(seconds) = value.as_i64() {
        return Some(Duration::seconds(seconds));
    }
    let text = value.as_str()?.trim();
    let (number, unit) = text.split_at(text.find(|c: char| c.is_alphabetic())?);
    let n: i64 = number.trim().parse().ok()?;
    match unit {
        "s" => Some(Duration::seconds(n)),
        "m" | "min" => Some(Duration::minutes(n)),
        "h" => Some(Duration::hours(n)),
        "d" => Some(Duration::days(n)),
        _ => None,
    }
}

pub(super) fn handle(api: &AdminApi, request: &HttpRequest, path: &str) -> Option<HttpResponse> {
    if path != "/__lsf/admin/clock" {
        return None;
    }
    let method = request.method.to_ascii_uppercase();
    let body: ClockBody = if method == "POST" {
        match serde_json::from_slice(&request.body) {
            Ok(body) => body,
            Err(_) if request.body.iter().all(u8::is_ascii_whitespace) => ClockBody::default(),
            Err(error) => {
                return Some(HttpResponse::json(
                    422,
                    &json!({"error": format!("the body is {{\"advance\": \"11m\"}} or {{\"at\": \"<RFC 3339>\"}}: {error}")}),
                ));
            }
        }
    } else {
        ClockBody {
            shop: request
                .query
                .iter()
                .find(|(name, _)| name == "shop")
                .map(|(_, value)| value.clone()),
            ..ClockBody::default()
        }
    };
    let domain = match &body.shop {
        Some(shop) => crate::auth::oauth::normalize_shop(shop),
        None => api.default_shop(),
    };
    if method == "GET" {
        return Some(match api.with_shop(&domain, |state| state.timestamp()) {
            Some(now) => HttpResponse::json(200, &json!({"shop": domain, "now": now})),
            None => {
                HttpResponse::json(404, &json!({"error": format!("there is no shop {domain}")}))
            }
        });
    }
    if method != "POST" {
        return Some(HttpResponse::json(
            405,
            &json!({"error": format!("{method} is not allowed here")}),
        ));
    }
    let target = |now: DateTime<Utc>| -> Result<DateTime<Utc>, String> {
        match (&body.at, &body.advance) {
            (Some(at), _) => DateTime::parse_from_rfc3339(at)
                .map(|at| at.with_timezone(&Utc))
                .map_err(|_| format!("`at` is not an RFC 3339 date: {at}")),
            (None, Some(advance)) => duration(advance).map(|step| now + step).ok_or_else(|| {
                format!("`advance` is not a duration (\"11m\", \"10d\"): {advance}")
            }),
            (None, None) => Err("give `advance` (\"11m\") or `at`".into()),
        }
    };
    let mut redacted = Vec::new();
    let moved = api.change_shop(&domain, |state| {
        let at = target(state.now())?;
        state.set_clock(Clock::Frozen(at));
        // The payloads of `customers/redact`, made before the data goes.
        let due = crate::domains::privacy::due_erasures(state);
        for customer in &due {
            if let Ok(payload) =
                super::samples::payload_of(state, "customers/redact", Some(customer))
            {
                redacted.push(payload);
            }
        }
        let erased = crate::domains::privacy::process_due_erasures(state);
        let deleted = crate::domains::checkouts::delete_expired(state);
        Ok::<_, String>((state.timestamp(), erased, deleted))
    });
    let Some(moved) = moved else {
        return Some(HttpResponse::json(
            404,
            &json!({"error": format!("there is no shop {domain}")}),
        ));
    };
    let (now, erased, deleted) = match moved {
        Ok(moved) => moved,
        Err(problem) => return Some(HttpResponse::json(422, &json!({"error": problem}))),
    };
    for payload in &redacted {
        api.webhooks().trigger(&crate::webhooks::Trigger {
            topic: "customers/redact",
            shop: &domain,
            payload,
            address: None,
            at: Utc::now(),
        });
    }
    Some(HttpResponse::json(
        200,
        &json!({"shop": domain, "now": now, "erased": erased, "deletedCheckouts": deleted}),
    ))
}
