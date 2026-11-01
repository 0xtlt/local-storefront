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
//! | `PUT /__lsf/admin/sources?url=` | Stubs a URL ([`crate::sources::Stubs`]): the body is what `themeCreate`, a `URL` body of `themeFilesUpsert` or `urlRedirectImportCreate` reads there from now on, never fetched. `{ok, url, size}`. |
//! | `GET /__lsf/admin/sources` | The stubbed URLs, `--admin-source` ones included: `{count, sources: [{url, size, from}]}`, `from` the file or `"request"`. `?url=` gives one's bytes. |
//! | `DELETE /__lsf/admin/sources?url=` | Forgets what requests stubbed, at `url` or everywhere (the `--admin-source` ones stay): `{ok, removed}`. |
//!
//! The shop is `?shop=` or the body's `shop`, else the default one; the stubbed URLs are every
//! shop's, as the internet is, and a reset keeps them. No token is needed: these are lsf's, for
//! the tests on this machine, as the other control endpoints are.

use chrono::{DateTime, Datelike, Duration, Utc};
use serde_json::{Value, json};

use super::{AdminApi, HttpRequest, HttpResponse};
use crate::domains::orders;
use crate::outbox::{self, Channel};
use crate::state::{Clock, ShopState, value};

/// The answer to a request of these endpoints, `None` for another path.
pub(super) fn handle(api: &AdminApi, request: &HttpRequest, path: &str) -> Option<HttpResponse> {
    let area = path.strip_prefix("/__lsf/admin/")?;
    let method = request.method.to_ascii_uppercase();
    if area == "sources" {
        // The body is a source's bytes, not JSON; no shop is named.
        return Some(sources(api, request, &method));
    }
    let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    let shop = request
        .query_param("shop")
        .map(str::to_string)
        .or_else(|| body.get("shop").and_then(Value::as_str).map(str::to_string))
        .map(|shop| crate::auth::oauth::normalize_shop(&shop))
        .unwrap_or_else(|| api.default_shop());
    let no_shop = || HttpResponse::error(404, format!("there is no shop {shop}"));
    Some(match (area, method.as_str()) {
        ("clock", "GET") => api
            .change_shop(&shop, |state| clock_json(state))
            .map_or_else(no_shop, |answer| HttpResponse::json(200, &answer)),
        ("clock", "POST") => {
            let target = match target_time(&body) {
                Ok(target) => target,
                Err(error) => return Some(HttpResponse::error(422, error)),
            };
            let moved = api.change_shop(&shop, |state| {
                let now = target(state.now()).ok_or_else(|| {
                    "the clock cannot move that far: it stays between the years 1 and 9999"
                        .to_string()
                })?;
                state.set_clock(Clock::Frozen(now));
                let ticked = api.tick(state);
                let erased: Vec<&Value> = ticked
                    .erased
                    .iter()
                    .filter_map(|customer| customer.get("id"))
                    .collect();
                let mut answer = clock_json(state);
                answer["ok"] = json!(true);
                answer["erased"] = json!(erased);
                answer["deletedCheckouts"] = json!(ticked.deleted_checkouts);
                Ok::<_, String>(answer)
            });
            let answer = match moved {
                None => return Some(no_shop()),
                Some(Err(error)) => {
                    return Some(HttpResponse::error(422, error));
                }
                Some(Ok(answer)) => answer,
            };
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
                        return Some(HttpResponse::error(
                            422,
                            format!("`channel` is one of {}", known.join(", ")),
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
                return Some(HttpResponse::error(
                    422,
                    "the body names the order: {\"order\": \"gid://shopify/Order/1\"}",
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
                Some(Err(error)) => HttpResponse::error(422, error),
            }
        }
        ("disputes", "POST") => {
            let Some(order) = body
                .get("order")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return Some(HttpResponse::error(
                    422,
                    "the body names the order: {\"order\": \"gid://shopify/Order/1\"}",
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
                Some(Err(error)) => HttpResponse::error(422, error),
            }
        }
        ("tax-app/activate", "POST") => match api.change_shop(&shop, orders::tax::activate) {
            None => no_shop(),
            Some(Ok(())) => {
                HttpResponse::json(200, &json!({"ok": true, "shop": shop, "state": "ACTIVE"}))
            }
            Some(Err(error)) => HttpResponse::error(409, error),
        },
        (path, "POST") if path.starts_with("disputes/") => {
            let rest = &path["disputes/".len()..];
            let (number, outcome) = rest.split_once('/').unwrap_or((rest, ""));
            let Ok(number) = number.parse::<u64>() else {
                return Some(HttpResponse::error(
                    404,
                    format!("no dispute `{number}`: /__lsf/admin/disputes/<number>/won"),
                ));
            };
            match api.change_shop(&shop, |state| {
                orders::shopify_payments::decide_dispute(state, number, outcome)
            }) {
                None => no_shop(),
                Some(Ok(dispute)) => {
                    HttpResponse::json(200, &json!({"ok": true, "shop": shop, "dispute": dispute}))
                }
                Some(Err(error)) => HttpResponse::error(409, error),
            }
        }
        (
            "clock" | "outbox" | "emails" | "orders/protect" | "orders/link-request" | "disputes",
            _,
        ) => HttpResponse::not_allowed(&method),
        _ => return None,
    })
}

fn sources(api: &AdminApi, request: &HttpRequest, method: &str) -> HttpResponse {
    let stubs = api.stubs();
    let url = request.query_param("url");
    match (method, url) {
        ("PUT", Some(url)) => {
            let size = request.body.len();
            match stubs.register(url, request.body.clone()) {
                Ok(url) => HttpResponse::json(200, &json!({"ok": true, "url": url, "size": size})),
                Err(error) => HttpResponse::error(422, error),
            }
        }
        ("PUT", None) => HttpResponse::error(
            422,
            "name the URL, encoded, in `?url=` (PUT /__lsf/admin/sources?url=https%3A%2F%2Fcdn.example.com%2Ftheme.zip), the file as the body",
        ),
        ("GET", None) => {
            let entries: Vec<Value> = stubs
                .list()
                .into_iter()
                .map(|entry| {
                    json!({
                        "url": entry.url,
                        "size": entry.size,
                        "from": match entry.from {
                            crate::sources::StubFrom::File(file) => json!(file.display().to_string()),
                            crate::sources::StubFrom::Request => json!("request"),
                        },
                    })
                })
                .collect();
            HttpResponse::json(200, &json!({"count": entries.len(), "sources": entries}))
        }
        ("GET", Some(url)) => match stubs.get(url) {
            Some(Ok(bytes)) => HttpResponse {
                status: 200,
                headers: vec![(
                    "content-type".to_string(),
                    "application/octet-stream".to_string(),
                )],
                body: bytes.to_vec(),
            },
            Some(Err(error)) => HttpResponse::error(422, error.to_string()),
            None => HttpResponse::error(404, format!("{url} is not stubbed")),
        },
        ("DELETE", Some(url)) => {
            let removed = usize::from(stubs.remove(url));
            HttpResponse::json(200, &json!({"ok": true, "removed": removed}))
        }
        ("DELETE", None) => HttpResponse::json(200, &json!({"ok": true, "removed": stubs.clear()})),
        _ => HttpResponse::not_allowed(method),
    }
}

/// `{shop, now, frozen}`.
fn clock_json(state: &ShopState) -> Value {
    json!({
        "shop": state.domain(),
        "now": value::date_time(state.now()),
        "frozen": matches!(state.clock(), Clock::Frozen(_)),
    })
}

/// The time a `POST /__lsf/admin/clock` body asks for, from the shop's time now: `None` when it
/// is past what a date and time can say (RFC 3339: the years 1 to 9999).
type Target = Box<dyn Fn(DateTime<Utc>) -> Option<DateTime<Utc>>>;

fn target_time(body: &Value) -> Result<Target, String> {
    // `now`, or `at`: the instant to move the clock to.
    if let Some(now) = body
        .get("now")
        .or_else(|| body.get("at"))
        .and_then(Value::as_str)
    {
        let at = value::parse_date_time(now)
            .ok_or_else(|| format!("`now` is not an RFC 3339 date and time: {now}"))?;
        return Ok(Box::new(move |_| Some(at)));
    }
    let advance = match body.get("advance") {
        Some(Value::Number(seconds)) => seconds
            .as_i64()
            .and_then(Duration::try_seconds)
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
    Ok(Box::new(move |now| {
        now.checked_add_signed(advance)
            .filter(|at| (1..=9999).contains(&at.year()))
    }))
}

/// `"30d"`, `"12h"`, `"15m"`, `"45s"`, `"90"` (seconds). `None` for another unit, or for more
/// than a duration can hold.
fn duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let (number, unit) = match text.char_indices().last() {
        Some((index, unit)) if unit.is_ascii_alphabetic() => (&text[..index], unit),
        _ => (text, 's'),
    };
    let number: i64 = number.trim().parse().ok()?;
    match unit {
        'd' => Duration::try_days(number),
        'h' => Duration::try_hours(number),
        'm' => Duration::try_minutes(number),
        's' => Duration::try_seconds(number),
        _ => None,
    }
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
        // More than a duration holds: refused, not a panic.
        assert_eq!(duration("9223372036854775807d"), None);
        assert_eq!(duration("9223372036854775807s"), None);
    }

    #[test]
    fn a_target_past_9999_is_refused() {
        let now = Utc::now();
        for body in [
            json!({"advance": "106751991167d"}),
            json!({"advance": "3000000d"}),
            json!({"advance": i64::MAX}),
            json!({"advance": "-800000d"}),
        ] {
            if let Ok(target) = target_time(&body) {
                assert_eq!(target(now), None, "{body}");
            }
        }
        let target = target_time(&json!({"advance": "1d"})).unwrap();
        assert_eq!(target(now), Some(now + Duration::days(1)));
    }
}
