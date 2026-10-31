//! The control endpoints of the world outside the shop, which a test drives: the shop's clock
//! (so that payment schedules come due), the outbox of what lsf records instead of sending it,
//! and what buyers and Shopify's services do to an order.
//!
//! | Request | What it does |
//! |---|---|
//! | `GET /__lsf/admin/clock?shop=` | The shop's time: `{shop, now, frozen}`. |
//! | `POST /__lsf/admin/clock` (`{"shop"?, "now"?, "advance"?}`) | Freezes the shop's clock at `now` (RFC 3339), or moves it on by `advance` (seconds, or `"30d"`, `"12h"`, `"15m"`, `"45s"`), then does what the time passing does: the payment schedules that came due fire `payment_schedules/due`. A reset puts the clock of the store data back. |
//! | `GET /__lsf/admin/outbox?shop=&channel=` | What lsf recorded instead of sending it ([`crate::outbox`]): `{shop, count, outbox}`, oldest first, of one channel (`email`, `carrier`, `flow`, `pos`, `data_erasure`) or all. |
//! | `DELETE /__lsf/admin/outbox?shop=&channel=` | Forgets them: `{ok, shop, outbox}`, how many. |
//! | `GET /__lsf/admin/emails?shop=`, `DELETE …` | The same for `channel=email`, as `{shop, count, emails}` and `{ok, shop, emails}`. |
//! | `POST /__lsf/admin/orders/protect` (`{"shop"?, "order", "status"?, "eligibility"?}`) | Shopify Protect changes its mind about an order: `Order.shopifyProtect` takes the status (`ACTIVE` by default) and the eligibility (`ELIGIBLE`), and `orders/shopify_protect_eligibility_changed` fires. |
//! | `POST /__lsf/admin/orders/link-request` (`{"shop"?, "order"}`) | The buyer asks for a new link to the order's status page: the email is recorded in the outbox, `orders/link_requested` fires. |
//! | `POST /__lsf/admin/disputes` (`{"shop"?, "order", "amount"?, "type"?, "reason"?}`) | The buyer's bank opens a dispute (a `CHARGEBACK` of what the order was paid, `FRAUDULENT`, by default): `disputes/create`. `201` `{ok, shop, dispute}`. |
//! | `POST /__lsf/admin/disputes/<n>/won`, `…/lost`, `…/accepted` | The bank decides it: `disputes/update`. `409` when it is decided already. |
//! | `POST /__lsf/admin/tax-app/activate` (`{"shop"?}`) | The merchant turns on the tax app the app made `READY` (`taxAppConfigure`): `ACTIVE`, `tax_services/update`. `409` otherwise. |
//!
//! The shop is `?shop=` or the body's `shop`, else the default one. No token is needed: these
//! are lsf's, for the tests on this machine, as the other control endpoints are.

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use super::{AdminApi, HttpRequest, HttpResponse};
use crate::domains::orders;
use crate::outbox::{self, Channel};
use crate::state::{Clock, ShopState, value};

/// The answer to a request of these endpoints, `None` for another path.
pub(super) fn handle(api: &AdminApi, request: &HttpRequest, path: &str) -> Option<HttpResponse> {
    let area = path.strip_prefix("/__lsf/admin/")?;
    let method = request.method.to_ascii_uppercase();
    let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    let shop = request
        .query_param("shop")
        .map(str::to_string)
        .or_else(|| body.get("shop").and_then(Value::as_str).map(str::to_string))
        .map(|shop| crate::auth::oauth::normalize_shop(&shop))
        .unwrap_or_else(|| api.default_shop());
    let no_shop = || HttpResponse::json(404, &json!({"error": format!("there is no shop {shop}")}));
    Some(match (area, method.as_str()) {
        ("clock", "GET") => api
            .change_shop(&shop, |state| clock_json(state))
            .map_or_else(no_shop, |answer| HttpResponse::json(200, &answer)),
        ("clock", "POST") => {
            let target = match target_time(&body) {
                Ok(target) => target,
                Err(error) => return Some(HttpResponse::json(422, &json!({ "error": error }))),
            };
            let moved = api.change_shop(&shop, |state| {
                let now = target(state.now());
                state.set_clock(Clock::Frozen(now));
                orders::tick(state);
                // The data erasures the clock made due, with the `customers/redact` payloads they
                // send, and the checkouts old enough to be deleted.
                let due = crate::domains::privacy::due_erasures(state);
                let mut redact = Vec::new();
                for customer in &due {
                    if let Ok(payload) =
                        super::samples::payload_of(state, "customers/redact", Some(customer))
                    {
                        redact.push(payload);
                    }
                }
                let erased = crate::domains::privacy::process_due_erasures(state);
                let deleted = crate::domains::checkouts::delete_expired(state);
                let mut answer = clock_json(state);
                answer["ok"] = json!(true);
                answer["erased"] = json!(erased);
                answer["deletedCheckouts"] = json!(deleted);
                (answer, redact)
            });
            let Some((answer, redact)) = moved else {
                return Some(no_shop());
            };
            for payload in &redact {
                api.webhooks().trigger(&crate::webhooks::Trigger {
                    topic: "customers/redact",
                    shop: &shop,
                    payload,
                    address: None,
                    at: Utc::now(),
                });
            }
            HttpResponse::json(200, &answer)
        }
        ("outbox" | "emails", "GET" | "DELETE") => {
            // `/emails` is the outbox of one channel, under the name of its list.
            let (list, channel) = if area == "emails" {
                ("emails", Some(Channel::Email))
            } else {
                match request.query_param("channel").map(Channel::parse) {
                    None => ("outbox", None),
                    Some(Some(channel)) => ("outbox", Some(channel)),
                    Some(None) => {
                        let known: Vec<&str> = Channel::ALL.map(Channel::as_str).to_vec();
                        return Some(HttpResponse::json(
                            422,
                            &json!({"error": format!("`channel` is one of {}", known.join(", "))}),
                        ));
                    }
                }
            };
            let answer = if method == "GET" {
                api.with_shop(&shop, |state| {
                    let entries = outbox::entries(state, channel);
                    json!({"shop": state.domain(), "count": entries.len(), list: entries})
                })
            } else {
                api.change_shop(&shop, |state| {
                    json!({"ok": true, "shop": state.domain(), list: outbox::clear(state, channel)})
                })
            };
            answer.map_or_else(no_shop, |answer| HttpResponse::json(200, &answer))
        }
        ("orders/protect" | "orders/link-request", "POST") => {
            let Some(order) = body
                .get("order")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return Some(HttpResponse::json(
                    422,
                    &json!({"error": "the body names the order: {\"order\": \"gid://shopify/Order/1\"}"}),
                ));
            };
            let protect = |name: &str, fallback: &str| {
                body.get(name)
                    .and_then(Value::as_str)
                    .unwrap_or(fallback)
                    .to_string()
            };
            let (status, eligibility) = (
                protect("status", "ACTIVE"),
                protect("eligibility", "ELIGIBLE"),
            );
            let done = api.change_shop(&shop, |state| {
                if area == "orders/protect" {
                    orders::risk::set_protect(state, &order, &status, &eligibility)
                } else {
                    orders::risk::request_link(state, &order)
                }
            });
            match done {
                None => no_shop(),
                Some(Ok(())) => {
                    HttpResponse::json(200, &json!({"ok": true, "shop": shop, "order": order}))
                }
                Some(Err(error)) => HttpResponse::json(422, &json!({ "error": error })),
            }
        }
        ("disputes", "POST") => {
            let Some(order) = body
                .get("order")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return Some(HttpResponse::json(
                    422,
                    &json!({"error": "the body names the order: {\"order\": \"gid://shopify/Order/1\"}"}),
                ));
            };
            let amount = body.get("amount").and_then(|amount| {
                orders::model::parse_cents(&amount.to_string().replace('"', ""))
            });
            let kind = body
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("CHARGEBACK")
                .to_ascii_uppercase();
            let reason = body
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("FRAUDULENT")
                .to_ascii_uppercase();
            let opened = api.change_shop(&shop, |state| {
                orders::shopify_payments::open_dispute(
                    state,
                    &orders::shopify_payments::NewDispute {
                        order: &order,
                        amount,
                        kind: &kind,
                        reason: &reason,
                    },
                )
            });
            match opened {
                None => no_shop(),
                Some(Ok(dispute)) => {
                    HttpResponse::json(201, &json!({"ok": true, "shop": shop, "dispute": dispute}))
                }
                Some(Err(error)) => HttpResponse::json(422, &json!({ "error": error })),
            }
        }
        ("tax-app/activate", "POST") => match api.change_shop(&shop, orders::tax::activate) {
            None => no_shop(),
            Some(Ok(())) => {
                HttpResponse::json(200, &json!({"ok": true, "shop": shop, "state": "ACTIVE"}))
            }
            Some(Err(error)) => HttpResponse::json(409, &json!({ "error": error })),
        },
        (path, "POST") if path.starts_with("disputes/") => {
            let rest = &path["disputes/".len()..];
            let (number, outcome) = rest.split_once('/').unwrap_or((rest, ""));
            let Ok(number) = number.parse::<u64>() else {
                return Some(HttpResponse::json(
                    404,
                    &json!({"error": format!("no dispute `{number}`: /__lsf/admin/disputes/<number>/won")}),
                ));
            };
            match api.change_shop(&shop, |state| {
                orders::shopify_payments::decide_dispute(state, number, outcome)
            }) {
                None => no_shop(),
                Some(Ok(dispute)) => {
                    HttpResponse::json(200, &json!({"ok": true, "shop": shop, "dispute": dispute}))
                }
                Some(Err(error)) => HttpResponse::json(409, &json!({ "error": error })),
            }
        }
        (
            "clock" | "outbox" | "emails" | "orders/protect" | "orders/link-request" | "disputes",
            _,
        ) => HttpResponse::json(
            405,
            &json!({"error": format!("{method} is not allowed here")}),
        ),
        _ => return None,
    })
}

/// `{shop, now, frozen}`.
fn clock_json(state: &ShopState) -> Value {
    json!({
        "shop": state.domain(),
        "now": value::date_time(state.now()),
        "frozen": matches!(state.clock(), Clock::Frozen(_)),
    })
}

/// The time a `POST /__lsf/admin/clock` body asks for, from the shop's time now.
type Target = Box<dyn Fn(DateTime<Utc>) -> DateTime<Utc>>;

fn target_time(body: &Value) -> Result<Target, String> {
    // `now`, or `at`: the instant to move the clock to.
    if let Some(now) = body
        .get("now")
        .or_else(|| body.get("at"))
        .and_then(Value::as_str)
    {
        let at = DateTime::parse_from_rfc3339(now)
            .map_err(|_| format!("`now` is not an RFC 3339 date and time: {now}"))?
            .with_timezone(&Utc);
        return Ok(Box::new(move |_| at));
    }
    let advance = match body.get("advance") {
        Some(Value::Number(seconds)) => seconds
            .as_i64()
            .map(Duration::seconds)
            .ok_or("`advance` is a whole number of seconds")?,
        Some(Value::String(text)) => duration(text).ok_or_else(|| {
            format!("`advance` is seconds or \"30d\", \"12h\", \"15m\", \"45s\": {text}")
        })?,
        _ => {
            return Err(
                "the body is {\"now\": \"2026-12-01T00:00:00Z\"} or {\"advance\": \"30d\"}"
                    .to_string(),
            );
        }
    };
    Ok(Box::new(move |now| now + advance))
}

/// `"30d"`, `"12h"`, `"15m"`, `"45s"`, `"90"` (seconds).
fn duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let (number, unit) = match text.char_indices().last() {
        Some((index, unit)) if unit.is_ascii_alphabetic() => (&text[..index], unit),
        _ => (text, 's'),
    };
    let number: i64 = number.trim().parse().ok()?;
    Some(match unit {
        'd' => Duration::days(number),
        'h' => Duration::hours(number),
        'm' => Duration::minutes(number),
        's' => Duration::seconds(number),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_durations() {
        assert_eq!(duration("30d"), Some(Duration::days(30)));
        assert_eq!(duration("12h"), Some(Duration::hours(12)));
        assert_eq!(duration("90"), Some(Duration::seconds(90)));
        assert_eq!(duration("3w"), None);
        assert_eq!(duration("d"), None);
    }
}
