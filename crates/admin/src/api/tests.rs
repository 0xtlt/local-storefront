//! The parts of the HTTP side that do not need a shop: reading a GraphQL request, the throttle
//! setting. `tests/api.rs` runs the endpoints end to end.

use serde_json::json;

use super::HttpRequest;
use super::admin::{ThrottleSetting, graphql_request};
use crate::throttle::Plan;

fn post(content_type: &str, body: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        path: "/admin/api/2026-10/graphql.json".into(),
        headers: [("content-type".to_string(), content_type.to_string())].into(),
        body: body.as_bytes().to_vec(),
        ..HttpRequest::default()
    }
}

#[test]
fn reads_the_graphql_request_of_every_form() {
    let json_body = graphql_request(&post(
        "application/json",
        r#"{"query": "query Q($n: Int) { shop { name } }", "variables": {"n": 1}, "operationName": "Q"}"#,
    ))
    .unwrap();
    assert_eq!(json_body.variables["n"], json!(1));
    assert_eq!(json_body.operation_name.as_deref(), Some("Q"));
    // Variables sent as a string of JSON, and an empty operation name.
    let string = graphql_request(&post(
        "application/json; charset=utf-8",
        r#"{"query": "{ shop { name } }", "variables": "{\"n\": 2}", "operationName": ""}"#,
    ))
    .unwrap();
    assert_eq!(string.variables["n"], json!(2));
    assert_eq!(string.operation_name, None);
    let raw = graphql_request(&post("application/graphql", "{ shop { name } }")).unwrap();
    assert_eq!(raw.query, "{ shop { name } }");
    let get = graphql_request(&HttpRequest {
        method: "GET".into(),
        query: vec![
            ("query".into(), "{ shop { name } }".into()),
            ("variables".into(), r#"{"a": true}"#.into()),
        ],
        ..HttpRequest::default()
    })
    .unwrap();
    assert_eq!(get.variables["a"], json!(true));

    assert!(graphql_request(&post("application/json", "{}")).is_err());
    assert!(graphql_request(&post("application/json", "not json")).is_err());
    assert!(
        graphql_request(&post(
            "application/json",
            r#"{"query": "{ a }", "variables": [1]}"#
        ))
        .is_err()
    );
}

#[test]
fn reads_the_throttle_setting() {
    assert_eq!(ThrottleSetting::parse("off"), Ok(ThrottleSetting::Off));
    assert_eq!(
        ThrottleSetting::parse("Plus"),
        Ok(ThrottleSetting::Plan(Plan::PLUS))
    );
    assert_eq!(ThrottleSetting::parse("store"), Ok(ThrottleSetting::Store));
    assert!(
        ThrottleSetting::parse("gold")
            .unwrap_err()
            .contains("off, store, standard")
    );
}
