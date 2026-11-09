//! The Partner API on lsf's origin ([`crate::admin::partner`]), and where a test finds its URL,
//! its token and its ids.
//!
//! | Request | What answers |
//! |---|---|
//! | `POST /{partner_id}/api/{version}/graphql.json` (`GET` with `?query=` too) | The Partner API: `X-Shopify-Access-Token` the organization's token, `{partner_id}` its Partner ID, `{version}` 2026-07, 2026-10 or 2027-01. A wrong token or Partner ID is Shopify's `401`, another version its `404`, a query over 50000 characters its `400`. |
//! | `GET /__lsf/admin/partner` | The URL, the Partner ID, the token, the versions, the app's GIDs and each shop's. |

use serde_json::{Value, json};

use super::admin::{graphql_request, origin_of};
use super::{AdminApi, HttpRequest, HttpResponse};
use crate::admin::domains::app as app_domain;
use crate::admin::partner::{self, AppView, Organization, ShopView};

/// An error of the Partner API, in Shopify's shape: `{"errors": [{"message", "extensions":
/// {"code": "<status>"}}]}`, with `X-Lsf-Reason` saying more when there is more to say.
fn refusal(status: u16, message: &str, reason: Option<String>) -> HttpResponse {
    let response = HttpResponse::json(
        status,
        &json!({"errors": [{"message": message, "extensions": {"code": status.to_string()}}]}),
    );
    match reason {
        Some(reason) => response.header("x-lsf-reason", reason),
        None => response,
    }
}

/// The versions lsf serves, as a sentence: `2026-07, 2026-10 and 2027-01`.
fn served() -> String {
    let (last, rest) = partner::VERSIONS
        .split_last()
        .expect("lsf serves a version of the Partner API");
    format!("{} and {last}", rest.join(", "))
}

/// `/{partner_id}/api/{version}/graphql.json`.
pub(super) fn handle(
    api: &AdminApi,
    request: &HttpRequest,
    partner_id: &str,
    version: &str,
) -> HttpResponse {
    let method = request.method.to_ascii_uppercase();
    if method != "POST" && method != "GET" {
        return refusal(
            405,
            &format!("{method} is not allowed: POST the query"),
            None,
        );
    }
    if !partner::serves(version) {
        let known = version == "unstable"
            || (version.len() == 7
                && version.as_bytes()[4] == b'-'
                && version
                    .bytes()
                    .enumerate()
                    .all(|(at, byte)| at == 4 || byte.is_ascii_digit()));
        let reason = format!(
            "lsf serves the Partner API versions {} (one schema, Shopify's 2026-10): not {version}",
            served()
        );
        return if known {
            refusal(404, &reason, None)
        } else {
            refusal(404, "Invalid API version", Some(reason))
        };
    }
    let expected_id = partner::partner_id(api.client_id());
    let expected_token = partner::token(api.client_id());
    let where_ = "GET /__lsf/admin/partner has the URL and the token";
    let given = request.header("x-shopify-access-token").map(str::trim);
    let refused = match given {
        None | Some("") => Some(format!(
            "no X-Shopify-Access-Token: the Partner API takes the token of a Partner API client ({where_})"
        )),
        Some(token) if token != expected_token => Some(format!(
            "X-Shopify-Access-Token is not the token of lsf's Partner API client ({where_})"
        )),
        Some(_) if partner_id != expected_id.to_string() => Some(format!(
            "{partner_id} is not the Partner ID of lsf's organization, {expected_id}: the path is /{expected_id}/api/{version}/graphql.json"
        )),
        Some(_) => None,
    };
    if let Some(reason) = refused {
        return refusal(401, "Invalid access token", Some(reason));
    }
    let query = match graphql_request(request) {
        Ok(query) => query,
        Err(reason) => return refusal(400, &reason, None),
    };
    if query.query.chars().count() > partner::MAX_QUERY_LENGTH {
        return refusal(
            400,
            &format!(
                "Maximum query length is {} characters",
                partner::MAX_QUERY_LENGTH
            ),
            None,
        );
    }
    let answer = partner::execute(&organization(api), &query);
    HttpResponse::json(200, &answer)
}

/// The organization as a request sees it now: the app, and each shop read in turn.
fn organization(api: &AdminApi) -> Organization {
    let legacy = api.pricing().is_none_or(|(pricing, _)| pricing.legacy);
    Organization {
        partner_id: partner::partner_id(api.client_id()),
        app: app_view(api),
        shops: api.read_shops(|state| ShopView::of(state, legacy)),
    }
}

fn app_view(api: &AdminApi) -> AppView {
    AppView {
        number: app_domain::app_id(api.app()),
        name: api
            .app()
            .and_then(|app| app.name.clone())
            .unwrap_or_else(|| app_domain::LOCAL_APP_TITLE.to_string()),
        api_key: api.client_id().to_string(),
    }
}

/// What `GET /__lsf/admin` says of the Partner API: its path, Partner ID and token.
pub(super) fn summary(api: &AdminApi) -> Value {
    let id = partner::partner_id(api.client_id());
    json!({
        "graphql": format!("/{id}/api/{}/graphql.json", partner::DEFAULT_VERSION),
        "partnerId": id,
        "token": partner::token(api.client_id()),
        "appId": format!("gid://shopify/App/{}", app_domain::app_id(api.app())),
    })
}

/// `GET /__lsf/admin/partner`.
pub(super) fn control(api: &AdminApi, request: &HttpRequest) -> HttpResponse {
    let method = request.method.to_ascii_uppercase();
    if method != "GET" {
        return HttpResponse::not_allowed(&method);
    }
    let id = partner::partner_id(api.client_id());
    let app = app_view(api);
    let shops: Vec<Value> = api.read_shops(|state| {
        let number = state.shop_gid().map_or(0, crate::admin::state::gid_number);
        json!({
            "shop": state.domain(),
            "id": format!("gid://shopify/Shop/{number}"),
            "partnersId": format!("gid://partners/Shop/{number}"),
        })
    });
    HttpResponse::json(
        200,
        &json!({
            "url": partner::endpoint(&origin_of(request), id),
            "partnerId": id,
            "token": partner::token(api.client_id()),
            "versions": partner::VERSIONS,
            "app": {
                "id": format!("gid://shopify/App/{}", app.number),
                "partnersId": format!("gid://partners/App/{}", app.number),
                "apiKey": app.api_key,
                "name": app.name,
            },
            "shops": shops,
        }),
    )
}
