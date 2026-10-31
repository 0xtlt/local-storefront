//! `audit_events/admin_api_activity`: "Triggers for each auditable Admin API request" (the
//! topic's description in 2026-10). Each GraphQL request lsf answers is one, its payload the
//! request (its query, variables, the names of its mutations, the codes of its errors) and how
//! it went (the status, the time it took, the versions asked for and served), in the keys of
//! the topic's example.
//!
//! Shopify allows "one active subscription per Plus store" to this topic, through Google Cloud
//! Pub/Sub or Amazon EventBridge, which lsf logs as not sent: the event is built only when a
//! subscription asks for it, and is no change of the shop.

use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};

use super::{AdminApi, HttpRequest};
use crate::auth::Grant;
use crate::graphql::Request;
use crate::graphql::ast::{OperationKind, Selection};
use crate::state::Event;
use crate::version::ApiVersion;
use crate::webhooks::payload::Records;

const TOPIC: &str = "audit_events/admin_api_activity";

/// The names of the root fields of a request's mutation, none for a query.
fn mutation_names(request: &Request) -> Vec<String> {
    let Ok(document) = crate::graphql::parse::parse(&request.query) else {
        return Vec::new();
    };
    let operation = document.operations().find(|operation| {
        request
            .operation_name
            .as_deref()
            .is_none_or(|name| operation.name == Some(name))
    });
    match operation {
        Some(operation) if operation.kind == OperationKind::Mutation => operation
            .selection_set
            .items
            .iter()
            .filter_map(|selection| match selection {
                Selection::Field(field) => Some(field.name.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The audit event of a GraphQL request, delivered to the subscriptions of the topic.
#[allow(clippy::too_many_arguments)]
pub(super) fn record(
    api: &AdminApi,
    http: &HttpRequest,
    grant: &Grant,
    request: &Request,
    answer: &Value,
    version: ApiVersion,
    took: Duration,
    app: Option<(String, String)>,
) {
    if !api.webhooks().subscribes(&grant.shop, TOPIC) {
        return;
    }
    let now = Utc::now();
    let codes: Vec<Value> = answer
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|error| error.pointer("/extensions/code").cloned())
        .collect();
    let requested = match crate::version::parse_path(&http.path) {
        Some(crate::version::AdminPath::Graphql(asked)) => asked.to_string(),
        _ => version.name().to_string(),
    };
    let (app_gid, app_name) = app.unwrap_or_default();
    let request_id = crate::webhooks::sign::new_id();
    let snapshot = crate::state::value::object(json!({
        "__typename": "AdminApiActivity",
        "time": now.timestamp(),
        "requestId": request_id,
        "clientIp": http
            .header("x-forwarded-for")
            .map(|forwarded| forwarded.split(',').next().unwrap_or_default().trim().to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string()),
        "responseTimeMs": took.as_secs_f64() * 1000.0,
        "userAgent": http.header("user-agent"),
        "failed": !codes.is_empty(),
        "responseCode": 200,
        "app": app_gid,
        "appName": app_name,
        "versionRequested": requested,
        "versionServed": version.name(),
        "userEmail": grant.user.as_ref().map(|user| user.email.clone()),
        "shop": grant.shop,
        "timestamp": crate::state::value::date_time(now),
        "mutationNames": mutation_names(request),
        "query": request.query,
        "variables": request.variables,
        "errorCodes": codes,
    }));
    let event = Event {
        topic: TOPIC.to_string(),
        shop: grant.shop.clone(),
        gid: format!("audit/{request_id}"),
        at: now,
        id: crate::webhooks::sign::new_id(),
        snapshot: Some(snapshot),
    };
    api.webhooks().emit(&event, &Records::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mutation_names_are_the_root_fields_of_the_operation() {
        let mut request = Request::new(
            "query Q { shop { name } } mutation M { a: segmentCreate(name: \"x\", query: \"y\") { segment { id } } tagsAdd(id: \"1\", tags: []) { node { id } } }",
        );
        request.operation_name = Some("M".into());
        assert_eq!(mutation_names(&request), ["segmentCreate", "tagsAdd"]);
        request.operation_name = Some("Q".into());
        assert!(mutation_names(&request).is_empty());
    }
}
