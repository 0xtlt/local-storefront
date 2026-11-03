//! Bulk operations: `bulkOperationRunQuery` and `bulkOperationRunMutation`, which lsf runs to
//! the end within the mutation, `bulkOperationCancel`, and the `bulkOperation`,
//! `bulkOperations` and `currentBulkOperation` queries.
//!
//! As shopify.dev's "Perform bulk operations with the GraphQL Admin API" (queries and imports)
//! describes them:
//!
//! - **A query** runs through the engine and the resolver against the shop, with the token's
//!   scopes, every page of every connection (no `first`, no limit of 250), and is written as JSON
//!   Lines: one line per node of a connection, a nested connection's nodes on lines of their own
//!   after their parent, with `__parentId` (the parent node's `id`), in the order of the
//!   connections. The query is checked first, as Shopify's restrictions say: at least one
//!   connection, five at most, nested two levels deep at most, not the top-level `node` or
//!   `nodes`, and the parent of a nested connection selects its `id`. A field the scopes do not
//!   cover fails the operation (`FAILED`, `ACCESS_DENIED`), with the lines written before in
//!   `partialDataUrl`.
//! - **A mutation** runs once per line of the JSONL its `stagedUploadPath` names (a staged upload,
//!   see [`super::uploads`]), the line's object being its variables; each result is a line of the
//!   output, `{"data": …, "__lineNumber": n}` (or the line's `errors`), from 0. Any mutation but
//!   the two bulk ones; a missing file is `NO_SUCH_FILE` with Shopify's message.
//! - **When**: at once. The payload answers the operation as Shopify does when it starts
//!   (`CREATED`, nothing counted yet); the next query reads it `COMPLETED` with its `url`,
//!   `objectCount`, `rootObjectCount` and `fileSize`, and `bulk_operations/finish` is delivered
//!   with the request. A query that gives no line has no `url` ("If the query produced no
//!   results, then the url field will return null"). So the limits (five running operations of a
//!   type, 2026-10's) are never reached, and `bulkOperationCancel` always finds the operation done.
//! - **The file** is served by lsf at `/__lsf/admin/bulk/<shop>/<n>.jsonl` (no token, as
//!   Shopify's signed URL needs none), kept until the shop is reset.
//!
//! Shopify's wording of the query checks is not published, except "Bulk queries must contain at
//! least one connection."; the others are lsf's, close to Shopify's restrictions as written.

use serde_json::{Map, Value, json};

use super::uploads;
use crate::admin::graphql::ast::{Field, Selection, SelectionSet};
use crate::admin::graphql::{self, FieldContext, FieldError, OperationKind, Output, Request};
use crate::admin::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::admin::schema::Schema;
use crate::admin::state::{Blob, ShopState, gid_number, value};

/// The most connections a bulk query may have.
const MAX_CONNECTIONS: usize = 5;

/// The deepest a connection may be nested in another.
const MAX_NESTING: usize = 2;

/// The largest JSONL of variables ("The size of the JSONL file can't exceed 100MB").
const MAX_VARIABLES_SIZE: usize = 100 * 1024 * 1024;

/// The message of a `stagedUploadPath` that names no upload.
const NO_SUCH_FILE: &str = "The JSONL file could not be found. Try uploading the file again, and check that you've entered the URL correctly for the stagedUploadPath mutation argument.";

pub fn register(registry: &mut Registry) {
    registry
        .query("currentBulkOperation", current_bulk_operation)
        .search_field("BulkOperation", "operation_type", |_r, _item, record| {
            Some(
                record
                    .get("type")
                    .and_then(Value::as_str)
                    .map(|kind| crate::admin::search::SearchValue::Keyword(kind.to_string()))
                    .into_iter()
                    .collect(),
            )
        })
        .mutation("bulkOperationRunQuery", run_query)
        .mutation("bulkOperationRunMutation", run_mutation)
        .mutation("bulkOperationCancel", cancel);
}

/// The key of the result of an operation among the shop's files.
pub fn result_key(number: u64) -> String {
    format!("bulk/{number}.jsonl")
}

/// The URL lsf serves the result of an operation at.
pub fn result_url(origin: &str, shop: &str, number: u64) -> String {
    format!("{origin}/__lsf/admin/bulk/{shop}/{number}.jsonl")
}

/// The file of `/__lsf/admin/bulk/<shop>/<file>`: `<n>.jsonl`.
pub fn result_file<'s>(state: &'s ShopState, file: &str) -> Option<&'s Blob> {
    let number: u64 = file.strip_suffix(".jsonl")?.parse().ok()?;
    state.blob(&result_key(number))
}

/// `currentBulkOperation(type:)`: the latest operation of the type.
fn current_bulk_operation(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let kind = ctx.arg_str("type").unwrap_or("QUERY");
    let state = r.state();
    let latest = state
        .keys("BulkOperation")
        .filter(|key| {
            state
                .get(key)
                .and_then(|record| record.get("type"))
                .and_then(Value::as_str)
                == Some(kind)
        })
        .max_by_key(|key| gid_number(key));
    Ok(Output::object_or_null(latest.map(Obj::record)))
}

// ---- checking a bulk query ------------------------------------------------------------------

/// What a walk of a bulk query found.
#[derive(Default)]
struct Shape {
    connections: usize,
    deepest: usize,
    problems: Vec<String>,
}

/// The fields of a selection set, through its fragments, with the type they are selected on.
fn fields<'d, 'q>(
    document: &'d graphql::ast::Document<'q>,
    set: &'d SelectionSet<'q>,
    type_name: &'d str,
    out: &mut Vec<(&'d Field<'q>, &'d str)>,
) {
    for item in &set.items {
        match item {
            Selection::Field(field) => out.push((field, type_name)),
            Selection::InlineFragment(fragment) => fields(
                document,
                &fragment.selection_set,
                fragment.type_condition.unwrap_or(type_name),
                out,
            ),
            Selection::FragmentSpread(spread) => {
                if let Some(fragment) = document.fragment(spread.name) {
                    fields(
                        document,
                        &fragment.selection_set,
                        fragment.type_condition,
                        out,
                    );
                }
            }
        }
    }
}

/// Whether the selection of a node selects its `id`, without an alias.
fn selects_id<'q>(
    document: &graphql::ast::Document<'q>,
    set: &SelectionSet<'q>,
    type_name: &str,
) -> bool {
    let mut found = Vec::new();
    fields(document, set, type_name, &mut found);
    found
        .iter()
        .any(|(field, _)| field.name == "id" && field.alias.is_none())
}

/// Walks the fields of `set` (on `type_name`), counting the connections and their depth.
fn walk<'q>(
    schema: &Schema,
    document: &graphql::ast::Document<'q>,
    set: &SelectionSet<'q>,
    type_name: &str,
    depth: usize,
    shape: &mut Shape,
) {
    let mut found = Vec::new();
    fields(document, set, type_name, &mut found);
    for (field, on) in found {
        let Some(definition) = schema.lookup_field(on, field.name) else {
            continue;
        };
        let named = definition.ty.named();
        if let Some(node_type) = schema
            .connection_node(named)
            .filter(|_| !definition.ty.is_list())
        {
            shape.connections += 1;
            shape.deepest = shape.deepest.max(depth);
            if depth > 0 && !selects_id(document, set, type_name) {
                shape.problems.push(format!(
                    "The parent 'node' field of the nested connection `{}` must select the 'id' field.",
                    field.response_key()
                ));
            }
            // `edges { node { … } }` or `nodes { … }`.
            let mut inner = Vec::new();
            fields(document, &field.selection_set, named, &mut inner);
            for (part, _) in inner {
                match part.name {
                    "edges" => {
                        let mut edge = Vec::new();
                        fields(document, &part.selection_set, "", &mut edge);
                        for (node, _) in edge.into_iter().filter(|(f, _)| f.name == "node") {
                            walk(
                                schema,
                                document,
                                &node.selection_set,
                                node_type,
                                depth + 1,
                                shape,
                            );
                        }
                    }
                    "nodes" => walk(
                        schema,
                        document,
                        &part.selection_set,
                        node_type,
                        depth + 1,
                        shape,
                    ),
                    _ => {}
                }
            }
        } else if !field.selection_set.is_empty() {
            walk(schema, document, &field.selection_set, named, depth, shape);
        }
    }
}

/// The problems of a bulk query: Shopify's restrictions.
fn check_query(schema: &Schema, query: &str) -> Vec<String> {
    let document = match graphql::parse::parse(query) {
        Ok(document) => document,
        Err(error) => return vec![format!("Invalid bulk query: {}", error.message)],
    };
    let operations: Vec<_> = document.operations().collect();
    let [operation] = operations.as_slice() else {
        return vec!["A bulk query has one operation.".to_string()];
    };
    if operation.kind != OperationKind::Query {
        return vec![
            "A bulk query is a query: run mutations with bulkOperationRunMutation.".to_string(),
        ];
    }
    let root = schema.query_type();
    let mut top = Vec::new();
    fields(&document, &operation.selection_set, root, &mut top);
    if top
        .iter()
        .any(|(field, _)| matches!(field.name, "node" | "nodes"))
    {
        return vec![
            "The top-level `node` and `nodes` fields can't be used in a bulk query.".to_string(),
        ];
    }
    let mut shape = Shape::default();
    walk(
        schema,
        &document,
        &operation.selection_set,
        root,
        0,
        &mut shape,
    );
    let mut problems = shape.problems;
    if shape.connections == 0 {
        problems.insert(
            0,
            "Bulk queries must contain at least one connection.".to_string(),
        );
    }
    if shape.connections > MAX_CONNECTIONS {
        problems.push(format!(
            "Bulk queries cannot contain more than {MAX_CONNECTIONS} connections."
        ));
    }
    if shape.deepest > MAX_NESTING {
        problems.push(format!(
            "Bulk queries cannot contain connections nested more than {MAX_NESTING} levels deep."
        ));
    }
    problems
}

// ---- writing the JSONL --------------------------------------------------------------------

/// The nodes of a connection's value (`{edges: [{node}]}` or `{nodes: […]}`), `None` when the
/// value is not a connection.
fn connection_nodes(value: &Value) -> Option<Vec<&Value>> {
    let map = value.as_object()?;
    if map.is_empty()
        || !map
            .keys()
            .all(|key| matches!(key.as_str(), "edges" | "nodes" | "pageInfo"))
    {
        return None;
    }
    if let Some(Value::Array(nodes)) = map.get("nodes") {
        return Some(nodes.iter().collect());
    }
    let edges = map.get("edges")?.as_array()?;
    Some(edges.iter().filter_map(|edge| edge.get("node")).collect())
}

/// Lifts the connections out of a value (a node, or an object inside one), into `children`
/// as `(nodes, parent)`.
fn lift<'v>(value: &'v Value, out: &mut Map<String, Value>, children: &mut Vec<&'v Value>) {
    let Some(map) = value.as_object() else {
        return;
    };
    for (key, field) in map {
        if let Some(nodes) = connection_nodes(field) {
            children.extend(nodes);
            continue;
        }
        match field {
            Value::Object(_) => {
                let mut inner = Map::new();
                lift(field, &mut inner, children);
                out.insert(key.clone(), Value::Object(inner));
            }
            other => {
                out.insert(key.clone(), other.clone());
            }
        }
    }
}

/// Writes a node and, after it, the nodes of its connections, depth first.
fn write_node(node: &Value, parent: Option<&str>, lines: &mut Vec<String>) {
    if node.is_null() {
        return;
    }
    let mut line = Map::new();
    let mut children = Vec::new();
    lift(node, &mut line, &mut children);
    if let Some(parent) = parent {
        line.insert("__parentId".into(), json!(parent));
    }
    lines.push(Value::Object(line).to_string());
    let id = node.get("id").and_then(Value::as_str).map(str::to_string);
    for child in children {
        write_node(child, id.as_deref(), lines);
    }
}

/// The lines of the data of a bulk query, and how many came from the root connections.
fn jsonl(data: &Value) -> (Vec<String>, usize) {
    let mut lines = Vec::new();
    let mut roots = 0;
    let Some(map) = data.as_object() else {
        return (lines, roots);
    };
    for field in map.values() {
        if let Some(nodes) = connection_nodes(field) {
            for node in nodes {
                roots += 1;
                write_node(node, None, &mut lines);
            }
        } else {
            let mut ignored = Map::new();
            let mut children = Vec::new();
            lift(field, &mut ignored, &mut children);
            for node in children {
                roots += 1;
                write_node(node, None, &mut lines);
            }
        }
    }
    (lines, roots)
}

// ---- the operations -----------------------------------------------------------------------

/// What an operation gave: its lines, how many are root objects, and its failure.
struct Outcome {
    lines: Vec<String>,
    roots: usize,
    error: Option<&'static str>,
}

/// Records a finished operation, keeps its file, emits `bulk_operations/finish`, and answers
/// the operation as it was when it started, for the payload.
fn finish(mutation: &mut Mutation<'_>, kind: &str, query: &str, outcome: Outcome) -> Value {
    let origin = mutation.origin().to_string();
    let shop = mutation.shop().to_string();
    let state = mutation.state_mut();
    let now = state.timestamp();
    let id = state.new_gid("BulkOperation");
    let number = gid_number(&id);
    let mut text = outcome.lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    let size = text.len();
    let has_lines = !outcome.lines.is_empty();
    if has_lines {
        state.put_blob(result_key(number), Blob::new(text, "application/jsonl"));
    }
    let url = has_lines.then(|| result_url(&origin, &shop, number));
    let failed = outcome.error.is_some();
    let created = json!({
        "__typename": "BulkOperation",
        "id": id,
        "status": "CREATED",
        "type": kind,
        "query": query,
        "errorCode": null,
        "createdAt": now,
        "completedAt": null,
        "objectCount": "0",
        "rootObjectCount": "0",
        "fileSize": null,
        "url": null,
        "partialDataUrl": null,
    });
    let mut record = value::object(created.clone());
    record.insert(
        "status".into(),
        json!(if failed { "FAILED" } else { "COMPLETED" }),
    );
    record.insert("errorCode".into(), json!(outcome.error));
    record.insert("completedAt".into(), json!(now));
    record.insert("objectCount".into(), json!(outcome.lines.len().to_string()));
    record.insert("rootObjectCount".into(), json!(outcome.roots.to_string()));
    record.insert(
        "fileSize".into(),
        if has_lines {
            json!(size.to_string())
        } else {
            Value::Null
        },
    );
    if failed {
        record.insert("partialDataUrl".into(), json!(url));
    } else {
        record.insert("url".into(), json!(url));
    }
    state.insert(record);
    state.emit("bulk_operations/finish", &id);
    created
}

fn run_query(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let query = ctx.arg_str("query").unwrap_or_default().to_string();
    let schema = ctx.schema();
    let problems = check_query(schema, &query);
    if !problems.is_empty() {
        let errors: Vec<UserError> = problems
            .into_iter()
            .map(|message| UserError::new(&["query"], message).code("INVALID"))
            .collect();
        return Ok(payload(json!({"bulkOperation": null}), &errors));
    }
    let prepared = match graphql::prepare(schema, &query, None, &Map::new()) {
        Ok(prepared) => prepared,
        Err(errors) => {
            let errors: Vec<UserError> = errors
                .iter()
                .map(|error| {
                    UserError::new(&["query"], format!("Invalid bulk query: {}", error.message))
                        .code("INVALID")
                })
                .collect();
            return Ok(payload(json!({"bulkOperation": null}), &errors));
        }
    };
    if ctx.arg_bool("groupObjects") == Some(true) {
        mutation.note(
            "bulkOperationRunQuery: lsf writes the default JSONL: `groupObjects` was ignored",
        );
    }
    let executed = {
        let mut resolver = mutation.resolver().with_bulk();
        prepared.execute(&mut resolver)
    };
    let error = executed.errors.first().map(|error| {
        let code = error
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .and_then(Value::as_str);
        if code == Some("ACCESS_DENIED") {
            "ACCESS_DENIED"
        } else {
            "INTERNAL_SERVER_ERROR"
        }
    });
    let (lines, roots) = jsonl(&executed.data);
    let created = finish(
        mutation,
        "QUERY",
        &query,
        Outcome {
            lines,
            roots,
            error,
        },
    );
    Ok(payload(json!({ "bulkOperation": created }), &[]))
}

/// The problems of the mutation of a bulk mutation: one mutation operation, not a bulk one.
fn check_mutation(query: &str) -> Option<String> {
    let document = match graphql::parse::parse(query) {
        Ok(document) => document,
        Err(error) => return Some(format!("Invalid mutation: {}", error.message)),
    };
    let operations: Vec<_> = document.operations().collect();
    let [operation] = operations.as_slice() else {
        return Some("The mutation of a bulk operation has one operation.".to_string());
    };
    if operation.kind != OperationKind::Mutation {
        return Some("The operation of bulkOperationRunMutation is a mutation.".to_string());
    }
    let mut top = Vec::new();
    fields(&document, &operation.selection_set, "Mutation", &mut top);
    if top.iter().any(|(field, _)| {
        matches!(
            field.name,
            "bulkOperationRunMutation" | "bulkOperationRunQuery"
        )
    }) {
        return Some(
            "bulkOperationRunMutation and bulkOperationRunQuery cannot run in a bulk operation."
                .to_string(),
        );
    }
    None
}

fn run_mutation(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let text = ctx.arg_str("mutation").unwrap_or_default().to_string();
    let path = ctx.arg_str("stagedUploadPath").unwrap_or_default();
    if let Some(problem) = check_mutation(&text) {
        let error = UserError::new(&["mutation"], problem).code("INVALID_MUTATION");
        return Ok(payload(json!({"bulkOperation": null}), &[error]));
    }
    let Some(file) = uploads::bytes(mutation.state(), path).cloned() else {
        let error = UserError::new(&["stagedUploadPath"], NO_SUCH_FILE).code("NO_SUCH_FILE");
        return Ok(payload(json!({"bulkOperation": null}), &[error]));
    };
    if file.bytes.len() > MAX_VARIABLES_SIZE {
        let error = UserError::new(
            &["stagedUploadPath"],
            "The size of the JSONL file can't exceed 100MB.",
        )
        .code("INVALID_STAGED_UPLOAD_FILE");
        return Ok(payload(json!({"bulkOperation": null}), &[error]));
    }
    if ctx.arg_str("clientIdentifier").is_some() {
        mutation.note("bulkOperationRunMutation: lsf does not keep `clientIdentifier`");
    }
    let mut lines = Vec::new();
    let content = String::from_utf8_lossy(&file.bytes).into_owned();
    for (number, line) in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        let mut result = match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(variables)) => {
                let request = Request {
                    query: text.clone(),
                    variables,
                    operation_name: None,
                };
                let (mut answer, _) = mutation.run_graphql(&request);
                if let Some(map) = answer.as_object_mut() {
                    map.remove("extensions");
                }
                answer
            }
            Ok(_) => {
                json!({"errors": [{"message": "A line of the JSONL file is an object of variables"}]})
            }
            Err(error) => json!({"errors": [{"message": format!("Invalid JSON: {error}")}]}),
        };
        if let Some(map) = result.as_object_mut() {
            map.insert("__lineNumber".into(), json!(number));
        }
        lines.push(result.to_string());
    }
    let roots = lines.len();
    let created = finish(
        mutation,
        "MUTATION",
        &text,
        Outcome {
            lines,
            roots,
            error: None,
        },
    );
    Ok(payload(json!({ "bulkOperation": created }), &[]))
}

/// `bulkOperationCancel`: lsf's operations are done when they are created, so there is nothing
/// to cancel.
fn cancel(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(record) = state.get_of(&id, "BulkOperation") else {
        let error = UserError::new(&["id"], "Bulk operation does not exist");
        return Ok(payload(json!({"bulkOperation": null}), &[error]));
    };
    let status = record
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let error = UserError::new(
        &[],
        format!("A bulk operation cannot be canceled when it is {status}"),
    );
    Ok(payload(json!({"bulkOperation": id}), &[error]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_queries_as_shopify_restricts_them() {
        let schema = crate::admin::schema::admin();
        assert!(
            check_query(
                schema,
                "{ products { edges { node { id variants { edges { node { id } } } } } } }"
            )
            .is_empty()
        );
        assert_eq!(
            check_query(schema, "{ shop { name } }"),
            ["Bulk queries must contain at least one connection."]
        );
        assert!(
            !check_query(schema, "{ node(id: \"gid://shopify/Product/1\") { id } }").is_empty()
        );
        let deep = "{ products { nodes { id variants { nodes { id metafields { nodes { id reference { ... on Product { collections { nodes { id } } } } } } } } } } }";
        assert!(
            check_query(schema, deep)
                .iter()
                .any(|problem| problem.contains("nested"))
        );
        let no_id = "{ products { nodes { title variants { nodes { id } } } } }";
        assert!(
            check_query(schema, no_id)
                .iter()
                .any(|problem| problem.contains("'id'"))
        );
    }

    #[test]
    fn writes_nested_connections_on_lines_of_their_own() {
        let data = json!({"products": {"edges": [
            {"node": {"id": "gid://shopify/Product/1", "title": "Hat",
                "seo": {"title": null},
                "variants": {"edges": [{"node": {"id": "gid://shopify/ProductVariant/11"}}]}}},
            {"node": {"id": "gid://shopify/Product/2", "title": "Cap", "variants": {"nodes": []}}},
        ]}});
        let (lines, roots) = jsonl(&data);
        assert_eq!(roots, 2);
        assert_eq!(
            lines,
            [
                r#"{"id":"gid://shopify/Product/1","title":"Hat","seo":{"title":null}}"#,
                // The fields in the order of the query, `__parentId` last, as shopify.dev's
                // example writes them.
                r#"{"id":"gid://shopify/ProductVariant/11","__parentId":"gid://shopify/Product/1"}"#,
                r#"{"id":"gid://shopify/Product/2","title":"Cap"}"#,
            ]
        );
    }

    #[test]
    fn names_the_result_of_an_operation() {
        assert_eq!(
            result_url("http://127.0.0.1:9292", "a.myshopify.com", 3),
            "http://127.0.0.1:9292/__lsf/admin/bulk/a.myshopify.com/3.jsonl"
        );
    }
}
