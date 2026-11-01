//! The control endpoints of the Subscriptions API: lsf's card vault, the log of what it did not
//! send nor charge, and the pages a customer opens from a billing attempt's challenge or a
//! payment method's update link.
//!
//! | Request | What it does |
//! |---|---|
//! | `POST /__lsf/admin/card-sessions` | The card vault: the body Shopify's vault takes, `{"credit_card": {"number", "first_name", "last_name", "month", "year", "verification_value"}}`, answers `201 {"id"}`, the `sessionId` of `customerPaymentMethodCreditCardCreate` and `…Update`. The card number decides what charging it does (`domains::subscriptions::payment_methods::card_outcome`). |
//! | `GET /__lsf/admin/subscriptions?shop=` | The log: the emails lsf did not send (`emails`), each billing attempt and what it charged (`charges`). |
//! | `GET /__lsf/admin/billing-attempts/<n>/challenge?shop=` | The 3D Secure page of a challenged attempt (`?format=json` for what it says): opening it makes the challenge `ON_SESSION_CHALLENGED`. |
//! | `POST /__lsf/admin/billing-attempts/<n>/challenge/complete`, `…/fail` (`?shop=`) | The customer passed or failed the challenge: the attempt bills (`subscription_billing_attempts/success`) or fails (`/failure`). `409` when it is not challenged. |
//! | `GET /__lsf/admin/payment-methods/<token>/update?shop=` | The page of the update link (`customerPaymentMethodGetUpdateUrl`, the update email). |
//! | `POST /__lsf/admin/payment-methods/<token>/update` (`?shop=`) | A new card for the method: `{"credit_card": {…}}` (or the page's form), as the vault takes it; `customer_payment_methods/update`. |
//!
//! None needs a token: they are the customer's and the vault's, which the app does not
//! authenticate either.

use serde_json::{Map, Value, json};

use super::{AdminApi, HttpRequest, HttpResponse, escape_html};
use crate::domains::subscriptions::{billing, payment_methods};
use crate::state::ShopState;

/// The answer to a request of the family's control endpoints, `None` for another path.
pub(super) fn handle(api: &AdminApi, request: &HttpRequest, path: &str) -> Option<HttpResponse> {
    let rest = path.strip_prefix("/__lsf/admin/")?;
    let (area, rest) = rest.split_once('/').unwrap_or((rest, ""));
    let method = request.method.to_ascii_uppercase();
    Some(match (area, method.as_str()) {
        ("card-sessions", "POST") if rest.is_empty() => card_session(request),
        ("card-sessions", _) if rest.is_empty() => HttpResponse::not_allowed(&method),
        ("subscriptions", "GET") if rest.is_empty() => log(api, request),
        ("subscriptions", _) if rest.is_empty() => HttpResponse::not_allowed(&method),
        ("billing-attempts", _) => challenge(api, request, rest),
        ("payment-methods", _) => update_page(api, request, rest),
        _ => return None,
    })
}

/// The fields of a form body (`application/x-www-form-urlencoded`).
fn form(request: &HttpRequest) -> Map<String, Value> {
    form_urlencoded::parse(&request.body)
        .map(|(key, value)| (key.into_owned(), Value::String(value.into_owned())))
        .collect()
}

fn from_form(request: &HttpRequest) -> bool {
    request
        .header("content-type")
        .is_some_and(|kind| kind.starts_with("application/x-www-form-urlencoded"))
}

/// The shop a request names (`?shop=`), else the default one.
fn shop_of(api: &AdminApi, request: &HttpRequest) -> String {
    request
        .query_param("shop")
        .map(crate::auth::oauth::normalize_shop)
        .unwrap_or_else(|| api.default_shop())
}

/// The card of a request: `{"credit_card": {…}}` in JSON, or the fields of a form.
fn card_of(request: &HttpRequest) -> Option<Map<String, Value>> {
    if from_form(request) {
        return Some(form(request));
    }
    let body: Value = serde_json::from_slice(&request.body).ok()?;
    body.get("credit_card")?.as_object().cloned()
}

/// `POST /__lsf/admin/card-sessions`.
fn card_session(request: &HttpRequest) -> HttpResponse {
    let Some(card) = card_of(request) else {
        return HttpResponse::error(
            422,
            "the body is {\"credit_card\": {\"number\", \"first_name\", \"last_name\", \"month\", \"year\", \"verification_value\"}}",
        );
    };
    match payment_methods::vault_session(&card) {
        Ok(id) => HttpResponse::json(201, &json!({"id": id})),
        Err(error) => HttpResponse::error(422, error),
    }
}

/// `GET /__lsf/admin/subscriptions`.
fn log(api: &AdminApi, request: &HttpRequest) -> HttpResponse {
    let shop = shop_of(api, request);
    let found = api.change_shop(&shop, |state: &mut ShopState| {
        json!({
            "shop": state.domain(),
            "emails": payment_methods::emails(state),
            "charges": billing::charges(state),
        })
    });
    match found {
        Some(log) => HttpResponse::json(200, &log),
        None => HttpResponse::error(404, format!("there is no shop {shop}")),
    }
}

/// `/__lsf/admin/billing-attempts/<n>/challenge[/complete|/fail]`.
fn challenge(api: &AdminApi, request: &HttpRequest, rest: &str) -> HttpResponse {
    let method = request.method.to_ascii_uppercase();
    let parts: Vec<&str> = rest.split('/').collect();
    let (Some(number), Some(&"challenge")) = (
        parts.first().and_then(|n| n.parse::<u64>().ok()),
        parts.get(1),
    ) else {
        return HttpResponse::error(
            404,
            format!("no control endpoint at /__lsf/admin/billing-attempts/{rest}"),
        );
    };
    let shop = shop_of(api, request);
    let action = parts.get(2).copied().unwrap_or("");
    let outcome = match (method.as_str(), action) {
        ("GET", "") => api.change_shop(&shop, |state| billing::open_challenge(state, number)),
        ("POST", "complete" | "fail") => api.change_shop(&shop, |state| {
            billing::complete_challenge(state, number, action == "complete")
        }),
        (_, "" | "complete" | "fail") => return HttpResponse::not_allowed(&method),
        _ => {
            return HttpResponse::error(
                404,
                format!("no control endpoint at /__lsf/admin/billing-attempts/{rest}"),
            );
        }
    };
    let Some(outcome) = outcome else {
        return HttpResponse::error(404, format!("there is no shop {shop}"));
    };
    match outcome {
        Ok(attempt) => {
            let state = attempt
                .get("state")
                .and_then(|state| state.get("__typename"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let summary = json!({
                "attempt": attempt.get("id"),
                "state": state,
                "order": attempt.get("order"),
                "errorCode": attempt.get("errorCode"),
            });
            if request.query_param("format") == Some("json")
                || (method == "POST" && !from_form(request))
            {
                return HttpResponse::json(200, &summary);
            }
            HttpResponse::html(200, challenge_page(&shop, number, &state))
        }
        Err(billing::ChallengeError::NotFound) => {
            HttpResponse::error(404, format!("{shop} has no billing attempt {number}"))
        }
        Err(billing::ChallengeError::NotChallenged) => HttpResponse::error(
            409,
            format!("billing attempt {number} is not waiting for a challenge"),
        ),
    }
}

/// The 3D Secure page: what a customer opens from the challenge's `nextActionUrl`.
fn challenge_page(shop: &str, number: u64, state: &str) -> String {
    let waiting = state == "SubscriptionBillingAttemptActionRequiredState";
    let mut body = format!(
        "<!doctype html><meta charset=utf-8><title>Confirm your payment</title><body style=\"font-family:system-ui;max-width:520px;margin:40px auto\"><h1>Confirm your payment</h1><p>{} · billing attempt {number} · local 3D Secure page of lsf</p>",
        escape_html(shop)
    );
    if waiting {
        for (action, label) in [
            ("complete", "Complete authentication"),
            ("fail", "Fail authentication"),
        ] {
            body.push_str(&format!(
                "<form method=post action=\"/__lsf/admin/billing-attempts/{number}/challenge/{action}?shop={}\"><button>{label}</button></form>",
                escape_html(shop)
            ));
        }
    } else {
        body.push_str(&format!("<p>The attempt is {}.</p>", escape_html(state)));
    }
    body
}

/// `/__lsf/admin/payment-methods/<token>/update`.
fn update_page(api: &AdminApi, request: &HttpRequest, rest: &str) -> HttpResponse {
    let method = request.method.to_ascii_uppercase();
    let Some(token) = rest
        .strip_suffix("/update")
        .filter(|token| !token.contains('/'))
    else {
        return HttpResponse::error(
            404,
            format!("no control endpoint at /__lsf/admin/payment-methods/{rest}"),
        );
    };
    let shop = shop_of(api, request);
    match method.as_str() {
        "GET" => HttpResponse::html(
            200,
            format!(
                "<!doctype html><meta charset=utf-8><title>Update your payment method</title><body style=\"font-family:system-ui;max-width:520px;margin:40px auto\"><h1>Update your payment method</h1><p>{} · local page of lsf</p><form method=post><p><input name=number placeholder=\"Card number\"></p><p><input name=month placeholder=MM> <input name=year placeholder=YYYY></p><p><input name=first_name placeholder=\"First name\"> <input name=last_name placeholder=\"Last name\"></p><button>Save card</button></form>",
                escape_html(&shop)
            ),
        ),
        "POST" => {
            let Some(card) = card_of(request) else {
                return HttpResponse::error(422, "the body is {\"credit_card\": {…}}");
            };
            let session = match payment_methods::vault_session(&card) {
                Ok(session) => session,
                Err(error) => return HttpResponse::error(422, error),
            };
            let updated = api.change_shop(&shop, |state| {
                payment_methods::update_from_session(state, token, &session)
            });
            match updated {
                Some(Ok(id)) if from_form(request) => HttpResponse::html(
                    200,
                    format!(
                        "<!doctype html><meta charset=utf-8><p>Your payment method {} is updated.</p>",
                        escape_html(&id)
                    ),
                ),
                Some(Ok(id)) => {
                    HttpResponse::json(200, &json!({"ok": true, "customerPaymentMethod": id}))
                }
                Some(Err(error)) => HttpResponse::error(404, error),
                None => HttpResponse::error(404, format!("there is no shop {shop}")),
            }
        }
        _ => HttpResponse::not_allowed(&method),
    }
}
