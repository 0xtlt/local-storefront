//! Introspection answers graphql-js's `getIntrospectionQuery()` (with descriptions,
//! `specifiedByURL`, `isOneOf`, `isRepeatable` and the deprecation of input values, as GraphiQL
//! and graphql-codegen send it) as Shopify does: the answer is compared with Shopify's own
//! introspection, from the pinned @shopify/dev-mcp (`tests/fixtures/2026-10/introspection.json`,
//! a few types, written by `tools/admin/admin_schema.mjs`).
//!
//! The whole schema can be compared too:
//!
//! ```text
//! node tools/admin/admin_schema.mjs --introspection /tmp/admin-2026-10.json
//! LSF_ADMIN_INTROSPECTION=/tmp/admin-2026-10.json cargo test -p lsf-admin --test introspection
//! ```

use lsf_admin::graphql::{self, FieldContext, FieldError, Output, Request, Resolver};
use lsf_admin::schema::admin;
use serde_json::{Value, json};

/// graphql-js 16's `getIntrospectionQuery({descriptions, specifiedByUrl, directiveIsRepeatable,
/// schemaDescription, inputValueDeprecation, oneOf})`, with the `kind` of the root types.
const INTROSPECTION_QUERY: &str = r#"
query IntrospectionQuery {
  __schema {
    description
    queryType { name kind }
    mutationType { name kind }
    subscriptionType { name kind }
    types { ...FullType }
    directives {
      name
      description
      isRepeatable
      locations
      args(includeDeprecated: true) { ...InputValue }
    }
  }
}

fragment FullType on __Type {
  kind
  name
  description
  specifiedByURL
  isOneOf
  fields(includeDeprecated: true) {
    name
    description
    args(includeDeprecated: true) { ...InputValue }
    type { ...TypeRef }
    isDeprecated
    deprecationReason
  }
  inputFields(includeDeprecated: true) { ...InputValue }
  interfaces { ...TypeRef }
  enumValues(includeDeprecated: true) {
    name
    description
    isDeprecated
    deprecationReason
  }
  possibleTypes { ...TypeRef }
}

fragment InputValue on __InputValue {
  name
  description
  type { ...TypeRef }
  defaultValue
  isDeprecated
  deprecationReason
}

fragment TypeRef on __Type {
  kind
  name
  ofType {
    kind
    name
    ofType {
      kind
      name
      ofType {
        kind
        name
        ofType {
          kind
          name
          ofType {
            kind
            name
            ofType {
              kind
              name
              ofType {
                kind
                name
                ofType {
                  kind
                  name
                }
              }
            }
          }
        }
      }
    }
  }
}
"#;

/// Answers nothing: introspection does not ask the resolver.
struct Nothing;

impl Resolver for Nothing {
    type Object = ();
    fn query_root(&mut self) {}
    fn mutation_root(&mut self) {}
    fn type_name(&self, _: &()) -> String {
        String::new()
    }
    fn field(&mut self, _: &FieldContext<'_>, _: &()) -> Result<Output<()>, FieldError> {
        Ok(Output::Null)
    }
}

fn introspect() -> Value {
    let answer = graphql::run(
        admin(),
        &mut Nothing,
        &Request::new(INTROSPECTION_QUERY),
        None,
    )
    .to_json();
    assert!(answer.get("errors").is_none(), "{:#}", answer["errors"]);
    answer["data"]["__schema"].clone()
}

/// Compares a type of Shopify's introspection with lsf's, field by field, so that a difference
/// says where it is.
///
/// Shopify's introspection lists, among the possible types of `Node` and `DisplayableError`,
/// four types it does not define (`ExchangeV2`, `PreviewInstallUserError`, ...: hidden from the
/// introspection of an app, but not from these lists). graphql-js's `buildClientSchema` refuses
/// a schema that names a type it does not have, so lsf leaves them out: `known` are the types
/// lsf's introspection has, and the comparison skips the others.
fn compare_type(expected: &Value, actual: Option<&Value>, known: &[&str]) {
    let name = expected["name"].as_str().unwrap();
    let Some(actual) = actual else {
        panic!("lsf's introspection has no type {name}");
    };
    for key in [
        "kind",
        "description",
        "specifiedByURL",
        "isOneOf",
        "interfaces",
    ] {
        assert_eq!(actual[key], expected[key], "{name}.{key}");
    }
    let possible = expected["possibleTypes"].as_array().map(|types| {
        types
            .iter()
            .filter(|t| known.contains(&t["name"].as_str().unwrap_or_default()))
            .cloned()
            .collect::<Vec<_>>()
    });
    assert_eq!(
        actual["possibleTypes"],
        json!(possible),
        "{name}.possibleTypes"
    );
    for key in ["fields", "inputFields", "enumValues"] {
        let (Some(expected), Some(actual)) = (expected[key].as_array(), actual[key].as_array())
        else {
            assert_eq!(actual[key], expected[key], "{name}.{key}");
            continue;
        };
        let names = |items: &[Value]| -> Vec<String> {
            items
                .iter()
                .map(|item| item["name"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(names(actual), names(expected), "the {key} of {name}");
        for (actual, expected) in actual.iter().zip(expected) {
            assert_eq!(actual, expected, "{name}.{}", expected["name"]);
        }
    }
}

#[test]
fn answers_the_introspection_query_as_shopify() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/2026-10/introspection.json")).unwrap();
    let schema = introspect();
    assert_eq!(schema["description"], Value::Null);
    assert_eq!(schema["queryType"], fixture["queryType"]);
    assert_eq!(schema["mutationType"], fixture["mutationType"]);
    assert_eq!(schema["subscriptionType"], Value::Null);
    assert_eq!(schema["directives"], fixture["directives"]);
    let types = schema["types"].as_array().unwrap();
    assert_eq!(types.len(), 3699);
    let known: Vec<&str> = types.iter().filter_map(|t| t["name"].as_str()).collect();
    let find = |name: &str| types.iter().find(|t| t["name"] == name);
    for expected in fixture["types"].as_array().unwrap() {
        compare_type(expected, find(expected["name"].as_str().unwrap()), &known);
    }
    // The root types, for the fields the fixture keeps.
    for expected in fixture["roots"].as_array().unwrap() {
        let actual = find(expected["name"].as_str().unwrap()).unwrap();
        for field in expected["fields"].as_array().unwrap() {
            let found = actual["fields"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["name"] == field["name"]);
            assert_eq!(found, Some(field), "{}.{}", expected["name"], field["name"]);
        }
    }
}

#[test]
fn compares_every_type_when_given_shopifys_introspection() {
    let Ok(path) = std::env::var("LSF_ADMIN_INTROSPECTION") else {
        return;
    };
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let schema = introspect();
    let names: Vec<&Value> = schema["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| &t["name"])
        .collect();
    let expected_names: Vec<&Value> = expected["typeNames"].as_array().unwrap().iter().collect();
    assert_eq!(names, expected_names, "the order of the types");
    assert_eq!(schema["directives"], expected["directives"]);
    let types = schema["types"].as_array().unwrap();
    let known: Vec<&str> = types.iter().filter_map(|t| t["name"].as_str()).collect();
    for (expected, actual) in expected["types"].as_array().unwrap().iter().zip(types) {
        compare_type(expected, Some(actual), &known);
    }
}

#[test]
fn answers_type_and_deprecations() {
    let request = Request::new(
        r#"{
          status: __type(name: "ProductStatus") { kind name enumValues { name } }
          missing: __type(name: "Nope") { name }
          product: __type(name: "Product") {
            all: fields(includeDeprecated: true) { name }
            current: fields { name isDeprecated }
          }
          list: __type(name: "QueryRoot") { fields { name type { kind ofType { kind name } } } }
        }"#,
    );
    let answer = graphql::run(admin(), &mut Nothing, &request, None).to_json();
    let data = &answer["data"];
    assert_eq!(
        data["status"],
        json!({"kind": "ENUM", "name": "ProductStatus", "enumValues": [
            {"name": "ACTIVE"}, {"name": "ARCHIVED"}, {"name": "DRAFT"}, {"name": "UNLISTED"}
        ]})
    );
    assert_eq!(data["missing"], Value::Null);
    let all = data["product"]["all"].as_array().unwrap().len();
    let current = data["product"]["current"].as_array().unwrap();
    assert!(current.len() < all, "Product has deprecated fields");
    assert!(current.iter().all(|f| f["isDeprecated"] == false));
    let products = data["list"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "products")
        .unwrap();
    assert_eq!(
        products["type"],
        json!({"kind": "NON_NULL", "ofType": {"kind": "OBJECT", "name": "ProductConnection"}})
    );
}
