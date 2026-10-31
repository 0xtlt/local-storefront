//! The control endpoints of the app family: where the merchant approves the app's charges, where
//! clients upload to staged upload targets, and where bulk operations' results are downloaded.
//!
//! | Request | What it does |
//! |---|---|
//! | `GET /__lsf/admin/charges/<n>?shop=` | The confirmation page of a subscription or a one-time purchase (`?format=json` for what it says). |
//! | `POST /__lsf/admin/charges/<n>/approve`, `…/decline` (`?shop=`, or `{"shop"}`) | The merchant's decision: from the page's form, a `303` to the charge's `returnUrl` with `charge_id`, as Shopify redirects; otherwise JSON `{ok, charge, redirect}`. `409` when the charge is not waiting. |
//! | `POST /__lsf/admin/uploads/<shop>` | A staged upload target's form (multipart, the `file` last, as Google Cloud Storage takes it): `201` with GCS's XML answer. |
//! | `PUT /__lsf/admin/uploads/<shop>/<key>` | A `PUT` target: the body is the file. `200`. |
//! | `GET /__lsf/admin/uploads/<shop>/<key>` | What was uploaded there. |
//! | `GET /__lsf/admin/bulk/<shop>/<n>.jsonl` | The result of a bulk operation. |
//!
//! None of these needs a token, as Shopify's signed URLs and its admin page need none from the
//! app: they are for the merchant's browser and the client's uploads on this machine.

use serde_json::{Value, json};

use super::admin::origin_of;
use super::{AdminApi, HttpRequest, HttpResponse};
use crate::domains::app::{billing, bulk, uploads};
use crate::state::ShopState;

/// The answer to a request of the app family's control endpoints, `None` for another path.
pub(super) fn handle(api: &AdminApi, request: &HttpRequest, path: &str) -> Option<HttpResponse> {
    let rest = path.strip_prefix("/__lsf/admin/")?;
    let (area, rest) = rest.split_once('/').unwrap_or((rest, ""));
    match area {
        "charges" => Some(charges(api, request, rest)),
        "uploads" => Some(uploads_endpoint(api, request, rest)),
        "bulk" => Some(bulk_result(api, request, rest)),
        _ => None,
    }
}

fn not_found(message: impl Into<String>) -> HttpResponse {
    HttpResponse::json(404, &json!({"error": message.into()}))
}

fn method_not_allowed(method: &str) -> HttpResponse {
    HttpResponse::json(
        405,
        &json!({"error": format!("{method} is not allowed here")}),
    )
}

/// The shop a request names (`?shop=`, a JSON or form body's `shop`), else the default one.
fn shop_of(api: &AdminApi, request: &HttpRequest) -> String {
    let from_body = || {
        let body = String::from_utf8_lossy(&request.body);
        serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| value.get("shop")?.as_str().map(str::to_string))
            .or_else(|| {
                form_urlencoded::parse(body.as_bytes())
                    .find(|(key, _)| key == "shop")
                    .map(|(_, value)| value.into_owned())
            })
    };
    request
        .query_param("shop")
        .map(str::to_string)
        .or_else(from_body)
        .map(|shop| crate::auth::oauth::normalize_shop(&shop))
        .unwrap_or_else(|| api.default_shop())
}

/// Runs `read` on a shop, `None` when there is no such shop.
fn with_shop<T>(api: &AdminApi, shop: &str, read: impl FnOnce(&mut ShopState) -> T) -> Option<T> {
    api.change_shop(shop, read)
}

// ---- charges ------------------------------------------------------------------------------

fn charges(api: &AdminApi, request: &HttpRequest, rest: &str) -> HttpResponse {
    let method = request.method.to_ascii_uppercase();
    let (number, action) = rest.split_once('/').unwrap_or((rest, ""));
    let Ok(number) = number.parse::<u64>() else {
        return not_found(format!(
            "no charge `{number}`: /__lsf/admin/charges/<number>"
        ));
    };
    let shop = shop_of(api, request);
    match (method.as_str(), action) {
        ("GET", "") => {
            let Some(found) = with_shop(api, &shop, |state| billing::charge(state, number)) else {
                return not_found(format!("there is no shop {shop}"));
            };
            let Some(charge) = found else {
                return not_found(format!("{shop} has no charge {number}"));
            };
            if request.query_param("format") == Some("json") {
                return HttpResponse::json(200, &charge.to_json());
            }
            html(200, &charge_page(&shop, &charge))
        }
        ("POST", "approve" | "decline") => {
            let decision = if action == "approve" {
                billing::Decision::Approve
            } else {
                billing::Decision::Decline
            };
            let Some(result) = with_shop(api, &shop, |state| {
                billing::decide(state, number, decision)
                    .map(|redirect| (redirect, billing::charge(state, number)))
            }) else {
                return not_found(format!("there is no shop {shop}"));
            };
            let from_page = request
                .header("content-type")
                .is_some_and(|kind| kind.starts_with("application/x-www-form-urlencoded"));
            match result {
                Ok((redirect, charge)) if from_page => {
                    let _ = charge;
                    HttpResponse {
                        status: 303,
                        headers: vec![("location".into(), redirect)],
                        body: Vec::new(),
                    }
                }
                Ok((redirect, charge)) => HttpResponse::json(
                    200,
                    &json!({
                        "ok": true,
                        "charge": charge.map(|charge| charge.to_json()),
                        "redirect": redirect,
                    }),
                ),
                Err(billing::DecisionError::NotFound) => {
                    not_found(format!("{shop} has no charge {number}"))
                }
                Err(billing::DecisionError::NotPending(status)) => HttpResponse::json(
                    409,
                    &json!({"error": format!("charge {number} is {status}: there is nothing to {action}")}),
                ),
            }
        }
        (_, "" | "approve" | "decline") => method_not_allowed(&method),
        _ => not_found(format!(
            "no control endpoint at /__lsf/admin/charges/{rest}"
        )),
    }
}

fn html(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![(
            "content-type".to_string(),
            "text/html; charset=utf-8".to_string(),
        )],
        body: body.as_bytes().to_vec(),
    }
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The look of lsf's pages (`checkout.css`'s tokens), inline: the Admin API serves this page
/// with or without an app, when the checkout's assets are not served.
const STYLE: &str = "\
:root{--text:#000;--subdued:#707070;--border:#dedede;--accent:#005bd1;--critical:#d91c1c;--radius:12px;\
--font:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif}\
*{box-sizing:border-box}body{margin:0;background:#f5f5f5;color:var(--text);font:400 14px/1.5 var(--font);\
-webkit-font-smoothing:antialiased}header{display:flex;align-items:center;gap:12px;padding:20px 32px;\
background:#fff;border-bottom:1px solid var(--border)}header strong{font-size:18px}header span{color:var(--subdued)}\
main{max-width:560px;margin:40px auto;padding:0 16px}.card{background:#fff;border:1px solid var(--border);\
border-radius:var(--radius);padding:24px;display:flex;flex-direction:column;gap:16px}h1{margin:0;font-size:21px}\
p{margin:0}.muted{color:var(--subdued)}ul{margin:0;padding-left:20px}.badge{display:inline-block;padding:2px 8px;\
border-radius:999px;background:#fff4d6;color:#5c4300;font-size:12px;font-weight:600}.status{font-weight:600}\
.actions{display:flex;gap:12px;justify-content:flex-end}form{margin:0}button{font:inherit;font-weight:600;\
padding:12px 20px;border-radius:var(--radius);border:1px solid var(--border);background:#fff;cursor:pointer}\
button.primary{background:var(--accent);border-color:var(--accent);color:#fff}a{color:var(--accent)}";

/// The confirmation page of a charge, in the look of lsf's pages.
fn charge_page(shop: &str, charge: &billing::Charge) -> String {
    let what = match (charge.kind, charge.pending_capped_amount.is_some()) {
        ("AppPurchaseOneTime", _) => "Approve a one-time charge",
        (_, true) => "Approve a new usage limit",
        _ => "Approve a subscription",
    };
    let mut body = format!(
        "<header><strong>{}</strong><span>{} · local charge page of lsf</span></header><main><div class=\"card\">",
        esc(&charge.app),
        esc(shop)
    );
    body.push_str(&format!("<h1>{what}</h1>"));
    body.push_str(&format!(
        "<p><strong>{}</strong>{}</p>",
        esc(&charge.name),
        if charge.test {
            " <span class=\"badge\">Test</span>"
        } else {
            ""
        }
    ));
    body.push_str("<ul>");
    for line in &charge.lines {
        body.push_str(&format!("<li>{}</li>", esc(line)));
    }
    body.push_str("</ul>");
    if let Some(new) = &charge.pending_capped_amount {
        body.push_str(&format!(
            "<p>The app asks to raise the usage limit to <strong>{}</strong>.</p>",
            esc(new)
        ));
    }
    if charge.trial_days > 0 {
        body.push_str(&format!(
            "<p class=\"muted\">Free for {} day{}, starting when you approve.</p>",
            charge.trial_days,
            if charge.trial_days == 1 { "" } else { "s" }
        ));
    }
    if charge.decidable() {
        let action = |name: &str| {
            format!(
                "/__lsf/admin/charges/{}/{name}?shop={}",
                charge.number,
                esc(shop)
            )
        };
        body.push_str(&format!(
            "<div class=\"actions\"><form method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"shop\" value=\"{}\"><button type=\"submit\">Decline</button></form>\
             <form method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"shop\" value=\"{}\"><button class=\"primary\" type=\"submit\">Approve</button></form></div>",
            action("decline"),
            esc(shop),
            action("approve"),
            esc(shop)
        ));
        body.push_str(
            "<p class=\"muted\">Either way you return to the app, with <code>charge_id</code>. No money moves: lsf simulates the approval Shopify asks of the merchant.</p>",
        );
    } else {
        body.push_str(&format!(
            "<p>This charge is <span class=\"status\">{}</span>: there is nothing to decide.</p><p><a href=\"{}\">Back to the app</a></p>",
            esc(&charge.status.to_ascii_lowercase()),
            esc(&charge.return_url)
        ));
    }
    body.push_str("</div></main>");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{} · {}</title><style>{STYLE}</style></head><body>{body}</body></html>",
        esc(what),
        esc(&charge.app)
    )
}

// ---- uploads ------------------------------------------------------------------------------

/// `/__lsf/admin/uploads/<shop>[/<key>]`.
fn uploads_endpoint(api: &AdminApi, request: &HttpRequest, rest: &str) -> HttpResponse {
    let method = request.method.to_ascii_uppercase();
    let (shop, key) = rest.split_once('/').unwrap_or((rest, ""));
    let shop = crate::auth::oauth::normalize_shop(shop);
    match (method.as_str(), key) {
        ("POST", "") => {
            let content_type = request.header("content-type").unwrap_or_default();
            let Some(parts) = uploads::multipart(content_type, &request.body) else {
                return HttpResponse::json(
                    400,
                    &json!({"error": "the body is a multipart/form-data form: the target's parameters, then `file`"}),
                );
            };
            let field = |name: &str| {
                parts
                    .iter()
                    .find(|part| part.name.eq_ignore_ascii_case(name))
                    .map(|part| String::from_utf8_lossy(&part.bytes).into_owned())
            };
            let Some(key) = field("key").filter(|key| !key.is_empty()) else {
                return HttpResponse::json(400, &json!({"error": "the form has no `key`"}));
            };
            let Some(file) = parts.iter().find(|part| part.name == "file") else {
                return HttpResponse::json(400, &json!({"error": "the form has no `file`"}));
            };
            let kind = field("Content-Type")
                .or_else(|| file.content_type.clone())
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let stored = with_shop(api, &shop, |state| {
                uploads::store(state, &key, file.bytes.clone(), &kind)
            });
            if stored.is_none() {
                return not_found(format!("there is no shop {shop}"));
            }
            let location = format!("{}/{key}", uploads::bucket_url(&origin_of(request), &shop));
            let xml = format!(
                "<?xml version='1.0' encoding='UTF-8'?><PostResponse><Location>{}</Location><Bucket>{}</Bucket><Key>{}</Key></PostResponse>",
                esc(&location),
                esc(&shop),
                esc(&key)
            );
            let status = if field("success_action_status").as_deref() == Some("201") {
                201
            } else {
                204
            };
            HttpResponse {
                status,
                headers: vec![("content-type".into(), "application/xml".into())],
                body: if status == 201 {
                    xml.into_bytes()
                } else {
                    Vec::new()
                },
            }
        }
        ("PUT", key) if !key.is_empty() => {
            let kind = request
                .header("content-type")
                .unwrap_or("application/octet-stream")
                .to_string();
            let stored = with_shop(api, &shop, |state| {
                uploads::store(state, key, request.body.clone(), &kind)
            });
            match stored {
                Some(()) => HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: Vec::new(),
                },
                None => not_found(format!("there is no shop {shop}")),
            }
        }
        ("GET", key) if !key.is_empty() => {
            let found = with_shop(api, &shop, |state| state.blob(key).cloned());
            match found.flatten() {
                Some(blob) => HttpResponse {
                    status: 200,
                    headers: vec![("content-type".into(), blob.content_type.clone())],
                    body: blob.bytes.to_vec(),
                },
                None => not_found(format!("nothing was uploaded to {key} of {shop}")),
            }
        }
        _ => method_not_allowed(&method),
    }
}

// ---- bulk operations ------------------------------------------------------------------------

/// `GET /__lsf/admin/bulk/<shop>/<n>.jsonl`.
fn bulk_result(api: &AdminApi, request: &HttpRequest, rest: &str) -> HttpResponse {
    let method = request.method.to_ascii_uppercase();
    if method != "GET" {
        return method_not_allowed(&method);
    }
    let (shop, file) = rest.split_once('/').unwrap_or((rest, ""));
    let shop = crate::auth::oauth::normalize_shop(shop);
    let found = with_shop(api, &shop, |state| bulk::result_file(state, file).cloned());
    match found.flatten() {
        Some(blob) => HttpResponse {
            status: 200,
            headers: vec![
                ("content-type".into(), "application/jsonl".into()),
                (
                    "content-disposition".into(),
                    format!("attachment; filename=\"bulk-{file}\""),
                ),
            ],
            body: blob.bytes.to_vec(),
        },
        None => not_found(format!("{shop} has no bulk operation result {file}")),
    }
}
