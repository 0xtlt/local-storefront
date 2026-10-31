//! The GraphQL engine against the real Admin schema, with a toy resolver: a shop and a few
//! products held as JSON, read field by field.

use std::sync::Arc;
use std::time::Duration;

use lsf_admin::graphql::{self, Bucket, FieldContext, FieldError, Output, Request, Resolver};
use lsf_admin::schema::{Kind, admin};
use lsf_admin::throttle::{ManualClock, Plan, Throttle};
use serde_json::{Value, json};

/// An object of the toy store: its type and its fields as JSON.
#[derive(Clone, Debug)]
struct Obj {
    ty: String,
    data: Value,
}

impl Obj {
    fn new(ty: &str, data: Value) -> Obj {
        Obj {
            ty: ty.to_string(),
            data,
        }
    }
}

struct Toy {
    products: Vec<Value>,
    collections: Vec<Value>,
    /// What the resolver of `products` saw selected below it.
    lookahead: Vec<String>,
}

impl Toy {
    fn new() -> Toy {
        let product = |id: u64, title: &str, status: &str| {
            json!({
                "id": format!("gid://shopify/Product/{id}"),
                "title": title,
                "handle": title.to_lowercase().replace(' ', "-"),
                "status": status,
                "tags": ["cotton"],
                "createdAt": "2026-01-01T00:00:00Z",
                "updatedAt": "2026-01-02T00:00:00Z",
            })
        };
        Toy {
            products: vec![
                product(1, "Organic shirt", "ACTIVE"),
                product(2, "Linen trousers", "DRAFT"),
                product(3, "Wool hat", "ACTIVE"),
            ],
            collections: vec![json!({
                "id": "gid://shopify/Collection/7",
                "title": "Summer",
                "handle": "summer",
                "updatedAt": "2026-01-03T00:00:00Z",
            })],
            lookahead: Vec::new(),
        }
    }

    fn find(&self, id: &str) -> Option<Obj> {
        let product = self.products.iter().find(|p| p["id"] == id);
        let collection = self.collections.iter().find(|c| c["id"] == id);
        product
            .map(|p| Obj::new("Product", p.clone()))
            .or_else(|| collection.map(|c| Obj::new("Collection", c.clone())))
    }

    /// A connection of products: the first `first` of them, as edges and nodes.
    fn connection(&self, ctx: &FieldContext<'_>) -> Obj {
        let first = ctx.arg_i64("first").unwrap_or(0) as usize;
        let page: Vec<&Value> = self.products.iter().take(first).collect();
        let edges: Vec<Value> = page
            .iter()
            .map(|p| json!({"cursor": format!("cursor:{}", p["id"].as_str().unwrap()), "node": p}))
            .collect();
        Obj::new(
            "ProductConnection",
            json!({
                "edges": edges,
                "nodes": page,
                "pageInfo": {
                    "hasNextPage": first < self.products.len(),
                    "hasPreviousPage": false,
                    "startCursor": edges.first().map(|e| e["cursor"].clone()),
                    "endCursor": edges.last().map(|e| e["cursor"].clone()),
                },
            }),
        )
    }
}

impl Resolver for Toy {
    type Object = Obj;

    fn query_root(&mut self) -> Obj {
        Obj::new("QueryRoot", Value::Null)
    }

    fn mutation_root(&mut self) -> Obj {
        Obj::new("Mutation", Value::Null)
    }

    fn type_name(&self, object: &Obj) -> String {
        object.ty.clone()
    }

    fn field(&mut self, ctx: &FieldContext<'_>, object: &Obj) -> Result<Output<Obj>, FieldError> {
        match (object.ty.as_str(), ctx.field_name()) {
            ("QueryRoot", "shop") => Ok(Output::Object(Obj::new(
                "Shop",
                json!({"name": "My shop", "currencyCode": "EUR", "email": "shop@example.com"}),
            ))),
            ("QueryRoot", "products") => {
                let selection = ctx.selection();
                self.lookahead = selection.names().iter().map(|n| n.to_string()).collect();
                let below: Vec<String> = selection
                    .field("edges")
                    .field("node")
                    .names()
                    .iter()
                    .map(|n| format!("edges.node.{n}"))
                    .collect();
                self.lookahead.extend(below);
                Ok(Output::Object(self.connection(ctx)))
            }
            ("QueryRoot", "product" | "node" | "collection") => Ok(Output::object_or_null(
                self.find(ctx.arg_str("id").unwrap()),
            )),
            ("QueryRoot", "nodes") => {
                let ids = ctx.arg("ids").unwrap().as_array().unwrap();
                Ok(Output::List(
                    ids.iter()
                        .map(|id| Output::object_or_null(self.find(id.as_str().unwrap())))
                        .collect(),
                ))
            }
            ("QueryRoot", "shopifyqlQuery") => {
                Err(FieldError::new("Access denied for shopifyqlQuery field.")
                    .with_code("ACCESS_DENIED")
                    .with_extension("requiredAccess", "`read_reports` access scope."))
            }
            ("Mutation", "productCreate") => {
                if let Some(idempotent) = ctx.directive("idempotent") {
                    self.lookahead
                        .push(format!("idempotent:{}", idempotent["key"]));
                }
                let input = ctx.arg("product").unwrap();
                let id = format!("gid://shopify/Product/{}", self.products.len() + 1);
                let product = json!({"id": id, "title": input["title"], "status": input.get("status").cloned().unwrap_or(json!("ACTIVE")), "tags": []});
                self.products.push(product.clone());
                Ok(Output::Object(Obj::new(
                    "ProductCreatePayload",
                    json!({"product": product, "userErrors": []}),
                )))
            }
            _ => Ok(read(ctx, object)),
        }
    }
}

/// A field of an object read from its JSON: an object of the field's type (or of its
/// `__typename` for an interface or a union), a list, or a value.
fn read(ctx: &FieldContext<'_>, object: &Obj) -> Output<Obj> {
    let value = object
        .data
        .get(ctx.field_name())
        .cloned()
        .unwrap_or(Value::Null);
    let schema = ctx.schema();
    let named = schema.get(ctx.ty().named()).unwrap();
    fn convert(value: Value, named: &lsf_admin::schema::TypeDef) -> Output<Obj> {
        match value {
            Value::Null => Output::Null,
            Value::Array(items) if !named.is_leaf() => {
                Output::List(items.into_iter().map(|item| convert(item, named)).collect())
            }
            Value::Object(_) if named.kind == Kind::Object => {
                Output::Object(Obj::new(&named.name, value))
            }
            Value::Object(_) if named.is_abstract() => {
                let ty = value["__typename"].as_str().unwrap_or_default().to_string();
                Output::Object(Obj { ty, data: value })
            }
            value => Output::Value(value),
        }
    }
    convert(value, named)
}

fn run(resolver: &mut Toy, query: &str, variables: Value) -> Value {
    let request = Request::new(query).with_variables(variables);
    graphql::run(admin(), resolver, &request, None).to_json()
}

fn query(query: &str) -> Value {
    run(&mut Toy::new(), query, json!({}))
}

#[test]
fn answers_the_shop() {
    assert_eq!(
        query("{ shop { name } }"),
        json!({
            "data": {"shop": {"name": "My shop"}},
            "extensions": {"cost": {
                "requestedQueryCost": 1,
                "actualQueryCost": 1,
                "throttleStatus": {"maximumAvailable": 2000.0, "currentlyAvailable": 2000, "restoreRate": 100.0}
            }}
        })
    );
}

#[test]
fn pages_a_connection() {
    let answer =
        query("{ products(first: 2) { edges { node { id title } } pageInfo { hasNextPage } } }");
    assert_eq!(
        answer["data"],
        json!({"products": {
            "edges": [
                {"node": {"id": "gid://shopify/Product/1", "title": "Organic shirt"}},
                {"node": {"id": "gid://shopify/Product/2", "title": "Linen trousers"}},
            ],
            "pageInfo": {"hasNextPage": true},
        }})
    );
    // 2 + 2 × 1: the edges and the page info are free.
    assert_eq!(answer["extensions"]["cost"]["requestedQueryCost"], 4);
    assert_eq!(answer["extensions"]["cost"]["actualQueryCost"], 4);
    // Asked for 10, given 3: the actual cost is what was returned.
    let answer = query("{ products(first: 10) { nodes { id } } }");
    assert_eq!(answer["extensions"]["cost"]["requestedQueryCost"], 12);
    assert_eq!(answer["extensions"]["cost"]["actualQueryCost"], 5);
}

#[test]
fn shopify_documented_costs() {
    // shopify.dev/docs/api/admin-graphql: `products(first: 1)` with a title costs 3.
    let answer = query("{ products(first: 1) { edges { node { title } } } }");
    assert_eq!(answer["extensions"]["cost"]["requestedQueryCost"], 3);
    assert_eq!(answer["extensions"]["cost"]["actualQueryCost"], 3);
    // Shopify's engineering post: a shop's scalars cost the shop's 1 point.
    let answer = query("{ shop { id name email currencyCode } }");
    assert_eq!(answer["extensions"]["cost"]["requestedQueryCost"], 1);
    // A connection of five costs 2 + 5.
    let answer = query("{ products(first: 5) { edges { cursor node { id } } } }");
    assert_eq!(answer["extensions"]["cost"]["requestedQueryCost"], 7);
    // Nested connections multiply.
    let answer = query(
        "{ products(first: 10) { edges { node { variants(first: 5) { edges { node { id } } } } } } }",
    );
    assert_eq!(
        answer["extensions"]["cost"]["requestedQueryCost"],
        2 + 10 * (1 + 2 + 5)
    );
    // A mutation costs 10, plus what its payload selects.
    let mut toy = Toy::new();
    let answer = run(
        &mut toy,
        "mutation { productCreate(product: {title: \"Cap\"}) { product { id } userErrors { message } } }",
        json!({}),
    );
    assert_eq!(answer["extensions"]["cost"]["requestedQueryCost"], 12);
    assert_eq!(answer["extensions"]["cost"]["actualQueryCost"], 11);
    // An interface costs the most of its possible types: here a product with a connection.
    let answer = query(
        "{ node(id: \"gid://shopify/Collection/7\") { id ... on Product { variants(first: 3) { nodes { id } } } } }",
    );
    assert_eq!(
        answer["extensions"]["cost"]["requestedQueryCost"],
        1 + 2 + 3
    );
    assert_eq!(answer["extensions"]["cost"]["actualQueryCost"], 1);
    // Introspection is free.
    let answer = query("{ __schema { queryType { name } } __typename }");
    assert_eq!(answer["extensions"]["cost"]["requestedQueryCost"], 0);
}

#[test]
fn resolves_interfaces_unions_fragments_and_aliases() {
    let answer = query(
        r#"query Mixed($withHandle: Boolean = false) {
          first: node(id: "gid://shopify/Product/1") { __typename id ...Titled }
          second: node(id: "gid://shopify/Collection/7") {
            __typename
            ... on Collection { title handle @include(if: $withHandle) }
            ... on Product { status }
          }
          missing: node(id: "gid://shopify/Product/99") { id }
          nodes(ids: ["gid://shopify/Product/3", "gid://shopify/Product/42"]) { id }
        }
        fragment Titled on Product { title status }"#,
    );
    assert_eq!(
        answer["data"],
        json!({
            "first": {"__typename": "Product", "id": "gid://shopify/Product/1", "title": "Organic shirt", "status": "ACTIVE"},
            "second": {"__typename": "Collection", "title": "Summer"},
            "missing": null,
            "nodes": [{"id": "gid://shopify/Product/3"}, null],
        })
    );
    let answer = run(
        &mut Toy::new(),
        "query($h: Boolean!) { node(id: \"gid://shopify/Collection/7\") { ... on Collection { handle @include(if: $h) title @skip(if: $h) } } }",
        json!({"h": true}),
    );
    assert_eq!(answer["data"], json!({"node": {"handle": "summer"}}));
    assert_eq!(
        query("{ __typename }")["data"],
        json!({"__typename": "QueryRoot"})
    );
}

#[test]
fn runs_mutations_one_after_the_other() {
    let mut toy = Toy::new();
    let answer = run(
        &mut toy,
        r#"mutation Create($title: String!) {
          a: productCreate(product: {title: $title}) { product { id title } userErrors { field message } }
          b: productCreate(product: {title: "Second", status: DRAFT}) { product { id status } }
        }"#,
        json!({"title": "First"}),
    );
    assert_eq!(
        answer["data"],
        json!({
            "a": {"product": {"id": "gid://shopify/Product/4", "title": "First"}, "userErrors": []},
            "b": {"product": {"id": "gid://shopify/Product/5", "status": "DRAFT"}},
        })
    );
    assert_eq!(toy.products.len(), 5);
    // A resolver reads the directives of its field.
    run(
        &mut toy,
        "mutation($k: String!) { productCreate(product: {title: \"x\"}) @idempotent(key: $k) { product { id } } }",
        json!({"k": "123e4567"}),
    );
    assert_eq!(toy.lookahead, ["idempotent:\"123e4567\""]);
}

#[test]
fn a_field_error_has_its_path_and_the_rest_is_answered() {
    let answer = query(
        "{ shop { name } shopifyqlQuery(query: \"FROM sales SHOW total\") { tableData { columns { name } } } }",
    );
    assert_eq!(
        answer["errors"],
        json!([{
            "message": "Access denied for shopifyqlQuery field.",
            "locations": [{"line": 1, "column": 17}],
            "path": ["shopifyqlQuery"],
            "extensions": {"code": "ACCESS_DENIED", "requiredAccess": "`read_reports` access scope."}
        }])
    );
    assert_eq!(
        answer["data"],
        json!({"shop": {"name": "My shop"}, "shopifyqlQuery": null})
    );
    // `errors` first, as Shopify writes it.
    let keys: Vec<&String> = answer.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["errors", "data", "extensions"]);
}

#[test]
fn a_null_in_a_non_null_field_goes_up_to_the_nearest_nullable_one() {
    let mut toy = Toy::new();
    toy.products[0]["title"] = Value::Null;
    let answer = run(
        &mut toy,
        "{ shop { name } product(id: \"gid://shopify/Product/1\") { id title } }",
        json!({}),
    );
    assert_eq!(
        answer["errors"],
        json!([{
            "message": "Cannot return null for non-nullable field Product.title",
            "locations": [{"line": 1, "column": 61}],
            "path": ["product", "title"],
        }])
    );
    assert_eq!(
        answer["data"],
        json!({"shop": {"name": "My shop"}, "product": null})
    );
    // Through non-null edges and a non-null connection, up to the root.
    let answer = run(
        &mut toy,
        "{ products(first: 1) { edges { node { title } } } }",
        json!({}),
    );
    assert_eq!(answer["data"], Value::Null);
    assert_eq!(
        answer["errors"][0]["path"],
        json!(["products", "edges", 0, "node", "title"])
    );
    // A list of non-null items.
    let mut toy = Toy::new();
    toy.products[1]["tags"] = json!(["a", null]);
    let answer = run(
        &mut toy,
        "{ product(id: \"gid://shopify/Product/2\") { tags } }",
        json!({}),
    );
    assert_eq!(
        answer["errors"][0]["message"],
        "Cannot return null for non-nullable element of type 'String!' for Product.tags"
    );
    assert_eq!(answer["errors"][0]["path"], json!(["product", "tags", 1]));
    assert_eq!(answer["data"], json!({"product": null}));
}

#[test]
fn a_resolver_answer_that_does_not_fit_is_an_error() {
    let mut toy = Toy::new();
    toy.products[0]["status"] = json!("SOLD");
    let answer = run(
        &mut toy,
        "{ product(id: \"gid://shopify/Product/1\") { status } }",
        json!({}),
    );
    assert_eq!(
        answer["errors"][0]["message"],
        "`Product.status` returned `\"SOLD\"` at `product.status`, but this isn't a valid value for `ProductStatus`. Update the field or resolver to return one of `ProductStatus`'s values instead."
    );
    assert_eq!(answer["data"], json!({"product": null}));
}

#[test]
fn invalid_variables_are_refused_as_graphql_ruby_refuses_them() {
    let answer = run(
        &mut Toy::new(),
        "query Q($id: ID!) { product(id: $id) { id } }",
        json!({}),
    );
    assert_eq!(
        answer,
        json!({"errors": [{
            "message": "Variable $id of type ID! was provided invalid value",
            "locations": [{"line": 1, "column": 9}],
            "extensions": {"value": null, "problems": [{"path": [], "explanation": "Expected value to not be null"}]}
        }]})
    );
    let answer = run(
        &mut Toy::new(),
        "query Q($id: ID!) { product(id: $id) { id } }",
        json!({"id": "123"}),
    );
    assert_eq!(
        answer["errors"][0]["extensions"],
        json!({"value": "123", "problems": [{"path": [], "explanation": "Invalid global id `123`", "message": "Invalid global id `123`"}]})
    );
    let answer = run(
        &mut Toy::new(),
        "mutation M($product: ProductCreateInput!) { productCreate(product: $product) { product { id } } }",
        json!({"product": {"title": 1, "nope": true}}),
    );
    assert_eq!(
        answer["errors"][0]["message"],
        "Variable $product of type ProductCreateInput! was provided invalid value for title (Could not coerce value 1 to String), nope (Field is not defined on ProductCreateInput)"
    );
    assert_eq!(
        answer["errors"][0]["extensions"]["problems"],
        json!([
            {"path": ["title"], "explanation": "Could not coerce value 1 to String"},
            {"path": ["nope"], "explanation": "Field is not defined on ProductCreateInput"},
        ])
    );
    // A value given for a list is a list of one.
    let answer = run(
        &mut Toy::new(),
        "query Q($ids: [ID!]!) { nodes(ids: $ids) { id } }",
        json!({"ids": "gid://shopify/Product/2"}),
    );
    assert_eq!(
        answer["data"],
        json!({"nodes": [{"id": "gid://shopify/Product/2"}]})
    );
}

#[test]
fn selects_the_operation_by_name() {
    let document = "query A { shop { name } } query B { shop { email } }";
    let mut toy = Toy::new();
    let answer = graphql::run(admin(), &mut toy, &Request::new(document), None).to_json();
    assert_eq!(
        answer,
        json!({"errors": [{"message": "An operation name is required"}]})
    );
    let request = Request::new(document).with_operation_name("C");
    let answer = graphql::run(admin(), &mut toy, &request, None).to_json();
    assert_eq!(
        answer,
        json!({"errors": [{"message": "No operation named \"C\""}]})
    );
    let request = Request::new(document).with_operation_name("B");
    let answer = graphql::run(admin(), &mut toy, &request, None).to_json();
    assert_eq!(
        answer["data"],
        json!({"shop": {"email": "shop@example.com"}})
    );
}

#[test]
fn parse_and_validation_errors_answer_only_errors() {
    assert_eq!(
        query("{ shop { name }"),
        json!({"errors": [{"message": "Unexpected end of document", "locations": []}]})
    );
    assert_eq!(
        query("{ shop { nam } }"),
        json!({"errors": [{
            "message": "Field 'nam' doesn't exist on type 'Shop'",
            "locations": [{"line": 1, "column": 10}],
            "path": ["query", "shop", "nam"],
            "extensions": {"code": "undefinedField", "typeName": "Shop", "fieldName": "nam"}
        }]})
    );
}

/// Fragments in fragments nest deeper than the parser sees: a chain of them past its limit is
/// refused as a query nested too deeply, not answered with its deepest selections left out. A
/// fragment spread many times at a shallow depth is fine.
#[test]
fn refuses_fragments_nested_too_deeply() {
    let mut chained = String::from("{ shop { ...F0 } }\n");
    for n in 0..5000 {
        chained.push_str(&format!("fragment F{n} on Shop {{ name ...F{} }}\n", n + 1));
    }
    chained.push_str("fragment F5000 on Shop { name }\n");
    let answer = query(&chained);
    assert_eq!(
        answer["errors"][0]["message"],
        json!("This query is too large to execute."),
        "{answer}"
    );
    assert!(answer.get("data").is_none(), "{answer}");

    let mut wide = String::from("{ shop { ");
    for _ in 0..2000 {
        wide.push_str("...N ");
    }
    wide.push_str("} }\nfragment N on Shop { name }");
    assert_eq!(
        query(&wide)["data"]["shop"]["name"],
        query("{ shop { name } }")["data"]["shop"]["name"]
    );
}

#[test]
fn refuses_a_query_over_the_single_query_limit() {
    let answer = query(
        "{ products(first: 250) { edges { node { variants(first: 250) { edges { node { id } } } } } } }",
    );
    let requested = 2 + 250 * (1 + 2 + 250);
    assert_eq!(
        answer,
        json!({
            "errors": [{
                "message": format!("Query cost is {requested}, which exceeds the single query max cost limit (1000).\n\nSee https://shopify.dev/concepts/about-apis/rate-limits for more information on how the\ncost of a query is calculated.\n\nTo query larger amounts of data with fewer limits, bulk operations should be used instead.\nSee https://shopify.dev/tutorials/perform-bulk-operations-with-admin-api for usage details.\n"),
                "extensions": {"code": "MAX_COST_EXCEEDED", "cost": requested, "maxCost": 1000, "documentation": "https://shopify.dev/api/usage/limits#rate-limits"}
            }],
            "extensions": {"cost": {
                "requestedQueryCost": requested,
                "actualQueryCost": null,
                "throttleStatus": {"maximumAvailable": 2000.0, "currentlyAvailable": 2000, "restoreRate": 100.0}
            }}
        })
    );
}

#[test]
fn throttles_a_shop_whose_bucket_is_empty() {
    let clock = Arc::new(ManualClock::default());
    let plan = Plan {
        maximum_available: 10.0,
        restore_rate: 1.0,
    };
    let throttle = Throttle::with_clock(plan, clock.clone());
    let key = lsf_admin::throttle::key("my-shop.myshopify.com", "app");
    let bucket = Some(Bucket {
        throttle: &throttle,
        key: &key,
    });
    let request = Request::new("{ products(first: 5) { nodes { id } } }");
    let mut toy = Toy::new();
    // 7 requested, 5 actual: 10 - 7 + 2.
    let answer = graphql::run(admin(), &mut toy, &request, bucket).to_json();
    assert_eq!(
        answer["extensions"]["cost"]["throttleStatus"]["currentlyAvailable"],
        5
    );
    let answer = graphql::run(admin(), &mut toy, &request, bucket).to_json();
    assert_eq!(
        answer,
        json!({
            "errors": [{"message": "Throttled", "extensions": {"code": "THROTTLED", "documentation": "https://shopify.dev/api/usage/rate-limits"}}],
            "extensions": {"cost": {
                "requestedQueryCost": 7,
                "actualQueryCost": null,
                "throttleStatus": {"maximumAvailable": 10.0, "currentlyAvailable": 5, "restoreRate": 1.0}
            }}
        })
    );
    clock.advance(Duration::from_secs(2));
    let answer = graphql::run(admin(), &mut toy, &request, bucket).to_json();
    assert!(answer.get("errors").is_none());
    assert_eq!(
        answer["extensions"]["cost"]["throttleStatus"]["currentlyAvailable"],
        2
    );
}

#[test]
fn a_resolver_can_look_ahead() {
    let mut toy = Toy::new();
    run(
        &mut toy,
        "{ products(first: 1) { edges { node { id ... on Product { title } } } pageInfo { hasNextPage } } }",
        json!({}),
    );
    assert_eq!(
        toy.lookahead,
        ["edges", "pageInfo", "edges.node.id", "edges.node.title"]
    );
}

#[test]
fn arguments_reach_the_resolver_coerced_with_their_defaults() {
    struct Arguments(Vec<Value>);
    impl Resolver for Arguments {
        type Object = ();
        fn query_root(&mut self) {}
        fn mutation_root(&mut self) {}
        fn type_name(&self, _: &()) -> String {
            String::new()
        }
        fn field(&mut self, ctx: &FieldContext<'_>, _: &()) -> Result<Output<()>, FieldError> {
            self.0.push(Value::Object(ctx.arguments().clone()));
            Ok(Output::Null)
        }
    }
    let mut resolver = Arguments(Vec::new());
    let request = Request::new(
        "query($q: String, $n: Int) { products(first: $n, query: $q, sortKey: TITLE) { nodes { id } } }",
    )
    .with_variables(json!({"n": 3}));
    graphql::run(admin(), &mut resolver, &request, None);
    // `query` is given a variable without a value: left out. `reverse` has its default.
    assert_eq!(
        resolver.0,
        [json!({"first": 3, "reverse": false, "sortKey": "TITLE"})]
    );
}

#[test]
fn the_queries_of_shopifys_app_template_validate() {
    // Shopify's React Router app template (`app/routes/app._index.tsx`) and shopify.dev's
    // `metafieldsSet` example.
    let cases = [
        (
            r#"mutation populateProduct($product: ProductCreateInput!) {
              productCreate(product: $product) {
                product { id title handle status variants(first: 10) { edges { node { id price barcode createdAt } } } }
              }
            }"#,
            json!({"product": {"title": "Red Snowboard"}}),
            10 + 1 + 2 + 10,
        ),
        (
            r#"mutation shopifyReactRouterTemplateUpdateVariant($productId: ID!, $variants: [ProductVariantsBulkInput!]!) {
              productVariantsBulkUpdate(productId: $productId, variants: $variants) {
                productVariants { id price barcode createdAt }
              }
            }"#,
            json!({
                "productId": "gid://shopify/Product/1",
                "variants": [{"id": "gid://shopify/ProductVariant/1", "price": "100.00"}]
            }),
            10 + 1,
        ),
        (
            r#"mutation MetafieldsSet($metafields: [MetafieldsSetInput!]!) {
              metafieldsSet(metafields: $metafields) {
                metafields { key namespace value createdAt updatedAt }
                userErrors { field message code }
              }
            }"#,
            json!({"metafields": [{
                "key": "nickname", "namespace": "my_fields", "ownerId": "gid://shopify/Customer/624407574",
                "type": "single_line_text_field", "value": "Big Tuna"
            }]}),
            10 + 1 + 1,
        ),
    ];
    for (query, variables, cost) in cases {
        let prepared = graphql::prepare(admin(), query, None, variables.as_object().unwrap())
            .unwrap_or_else(|errors| panic!("{query}: {errors:#?}"));
        assert_eq!(prepared.requested_cost(), cost, "{query}");
    }
}
