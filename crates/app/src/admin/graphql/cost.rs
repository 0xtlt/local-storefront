//! The calculated query cost of the Admin API, as shopify.dev documents it ("GraphQL Admin
//! API rate limits", read in 2026-10, with the older versions of the page on the Web Archive and
//! Shopify's engineering post "Rate Limiting GraphQL APIs by Calculating Query Complexity"):
//!
//! - a scalar or an enum costs 0, an object 1, a mutation 10 (plus what its payload selects);
//! - a connection costs 2 plus `first` (or `last`) times the cost of one of its nodes: the edges
//!   and `cursor` are free, and so is `pageInfo`. `products(first: 1) { edges { node { title } } }`
//!   costs 3, the example of shopify.dev;
//! - an interface or a union costs the most of what its possible types select;
//! - a list that is not a connection costs what one of its items costs.
//!
//! The **requested** cost is computed from the query and the variables before it runs; the
//! **actual** cost, by the executor, from what was returned: the nodes a connection really has,
//! nothing for an object that is `null`. Introspection and `__typename` are free. A query whose
//! requested cost is over [`MAX_COST`] does not run: [`max_cost_exceeded`] is Shopify's error.
//!
//! What shopify.dev does not say, and lsf chooses: a connection given neither `first` nor `last`
//! costs 2 (Shopify refuses it as it runs); given both, the larger one counts. Shopify "reserves
//! the right to set manual costs on fields": lsf knows none of them.

use serde_json::{Value, json};

use super::ast::{Field, Operation, OperationKind, SelectionSet};
use super::error::Error;
use super::execute::{self, Context};
use super::parse::MAX_DEPTH;
use crate::admin::schema::{Field as FieldDef, Schema, TypeDef};

/// The most a single query may cost, whatever the plan.
pub const MAX_COST: u64 = 1000;

/// How a field adds up to the cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shape {
    /// What the field costs whatever it returns: 10 for a mutation.
    pub fixed: u64,
    /// What each object it returns costs by itself: 1, 2 for a connection, 0 for the edges and
    /// the page info of a connection.
    pub per_object: u64,
    /// Whether the items of a list add up (the `edges` and `nodes` of a connection), or the list
    /// costs what its costliest item does.
    pub sum_items: bool,
    /// Whether what the objects select is free (`pageInfo`).
    pub free_children: bool,
}

impl Shape {
    pub fn free() -> Shape {
        Shape {
            fixed: 0,
            per_object: 0,
            sum_items: false,
            free_children: true,
        }
    }
}

/// The shape of a field of a type: `mutation_root` for the root fields of a mutation.
pub(crate) fn shape(schema: &Schema, parent: &str, def: &FieldDef, mutation_root: bool) -> Shape {
    let object = |per_object, sum_items| Shape {
        fixed: 0,
        per_object,
        sum_items,
        free_children: false,
    };
    if mutation_root {
        return Shape {
            fixed: 10,
            ..object(0, false)
        };
    }
    let named = def.ty.named();
    if schema.get(named).is_none_or(TypeDef::is_leaf) {
        return Shape::free();
    }
    if schema.is_connection(named) {
        return object(2, false);
    }
    if schema.is_connection(parent) {
        match def.name.as_str() {
            "edges" => return object(0, true),
            "nodes" => return object(1, true),
            "pageInfo" => return Shape::free(),
            _ => {}
        }
    }
    object(1, false)
}

/// The requested cost of an operation.
pub(crate) fn requested(context: &Context<'_>, operation: &Operation<'_>) -> u64 {
    walk(context, operation, None)
}

/// The fields of an operation's requested cost, as Shopify lists them in `extensions.cost
/// .fields` for a request with `Shopify-GraphQL-Cost-Debug: 1` (shopify.dev, "GraphQL Admin API
/// rate limits", "Query cost debugging"): each field's `path` (its response keys), its
/// `definedCost` (what one of its objects costs by itself), its `requestedTotalCost` (what it
/// adds to the query, its children and the page sizes above it included) and its
/// `requestedChildrenCost` (what its children add, `null` for a scalar), the children before
/// their field. An interface or a union lists what its costliest possible type selects.
pub(crate) fn requested_fields(context: &Context<'_>, operation: &Operation<'_>) -> Vec<Value> {
    let mut report = Report {
        path: Vec::new(),
        multiplier: 1,
        fields: Vec::new(),
    };
    walk(context, operation, Some(&mut report));
    report.fields
}

/// What [`requested_fields`] collects as the cost is computed: where the walk is, how many
/// times the objects there are counted, and the fields so far.
struct Report {
    path: Vec<String>,
    multiplier: u64,
    fields: Vec<Value>,
}

fn walk(context: &Context<'_>, operation: &Operation<'_>, report: Option<&mut Report>) -> u64 {
    let schema = context.schema;
    let (root, mutation) = match operation.kind {
        OperationKind::Mutation => (schema.mutation_type(), true),
        _ => (Some(schema.query_type()), false),
    };
    let Some(root) = root.and_then(|name| schema.get(name)) else {
        return 0;
    };
    selection(context, root, &[&operation.selection_set], mutation, 1, 0, report)
}

/// What a selection costs on an object of a type: for an interface or a union, the most it
/// costs on one of its possible types.
fn selection<'a>(
    context: &Context<'a>,
    ty: &TypeDef,
    sets: &[&'a SelectionSet<'a>],
    mutation_root: bool,
    items: u64,
    depth: usize,
    report: Option<&mut Report>,
) -> u64 {
    // Deeper than a query may nest (through fragments), the executor refuses it.
    if depth > MAX_DEPTH {
        return 0;
    }
    if !ty.is_abstract() {
        return object(context, ty, sets, mutation_root, items, depth, report);
    }
    let costliest = ty
        .possible_types
        .iter()
        .filter_map(|name| context.schema.get(name))
        .map(|possible| {
            let cost = object(context, possible, sets, false, items, depth, None);
            (cost, possible)
        })
        .max_by_key(|(cost, _)| *cost);
    match (costliest, report) {
        (Some((_, possible)), Some(report)) => {
            object(context, possible, sets, false, items, depth, Some(report))
        }
        (Some((cost, _)), None) => cost,
        (None, _) => 0,
    }
}

fn object<'a>(
    context: &Context<'a>,
    ty: &TypeDef,
    sets: &[&'a SelectionSet<'a>],
    mutation_root: bool,
    items: u64,
    depth: usize,
    mut report: Option<&mut Report>,
) -> u64 {
    let mut total = 0;
    for nodes in execute::collect_fields(context, Some(&ty.name), sets).values() {
        total += field(
            context,
            ty,
            nodes,
            mutation_root,
            items,
            depth,
            report.as_deref_mut(),
        );
    }
    total
}

/// The requested cost of a field of an object. `items` is the size of the connection the
/// object is, which its `edges` and `nodes` are multiplied by.
fn field<'a>(
    context: &Context<'a>,
    parent: &TypeDef,
    nodes: &[&'a Field<'a>],
    mutation_root: bool,
    items: u64,
    depth: usize,
    mut report: Option<&mut Report>,
) -> u64 {
    let schema = context.schema;
    let node = nodes[0];
    if node.name.starts_with("__") {
        return 0;
    }
    let Some(def) = schema.lookup_field(&parent.name, node.name) else {
        return 0;
    };
    let shape = shape(schema, &parent.name, def, mutation_root);
    // Where the report is: this field's path, and how often its objects are counted.
    let outer = report.as_deref_mut().map(|report| {
        report.path.push(node.response_key().to_string());
        let outer = (report.multiplier, report.fields.len());
        if shape.sum_items {
            report.multiplier = report.multiplier.saturating_mul(items);
        }
        outer
    });
    let leaf = schema.get(def.ty.named()).is_none_or(TypeDef::is_leaf);
    let cost = match schema.get(def.ty.named()).filter(|ty| !ty.is_leaf()) {
        None => shape.fixed,
        Some(ty) => {
            let size = if schema.is_connection(&ty.name) {
                page_size(context, def, node)
            } else {
                1
            };
            let children = if shape.free_children {
                0
            } else {
                let sets: Vec<&'a SelectionSet<'a>> =
                    nodes.iter().map(|n| &n.selection_set).collect();
                selection(context, ty, &sets, false, size, depth + 1, report.as_deref_mut())
            };
            let one = shape.per_object + children;
            shape.fixed + if shape.sum_items { one * items } else { one }
        }
    };
    if let (Some(report), Some((multiplier, first_child))) = (report, outer) {
        let children: Option<u64> = (!leaf).then(|| {
            report.fields[first_child..]
                .iter()
                .filter(|entry| {
                    entry["path"]
                        .as_array()
                        .is_some_and(|path| path.len() == report.path.len() + 1)
                })
                .filter_map(|entry| entry["requestedTotalCost"].as_u64())
                .sum()
        });
        report.fields.push(json!({
            "path": report.path,
            "definedCost": shape.fixed + shape.per_object,
            "requestedTotalCost": cost.saturating_mul(multiplier),
            "requestedChildrenCost": children,
        }));
        report.path.pop();
        report.multiplier = multiplier;
    }
    cost
}

/// The number of nodes a connection field asks for: `first` or `last`.
fn page_size(context: &Context<'_>, def: &FieldDef, node: &Field<'_>) -> u64 {
    let arguments = execute::arguments(context, def, node);
    ["first", "last"]
        .iter()
        .filter_map(|name| arguments.get(*name)?.as_u64())
        .max()
        .unwrap_or(0)
}

/// Shopify's error for a query whose requested cost is over [`MAX_COST`], word for word as the
/// Admin API reference shows it (shopify.dev/docs/api/admin-graphql, "Sample 200 error
/// responses"), links included.
pub fn max_cost_exceeded(requested: u64) -> Error {
    let message = format!(
        "Query cost is {requested}, which exceeds the single query max cost limit ({MAX_COST}).\n\n\
See https://shopify.dev/concepts/about-apis/rate-limits for more information on how the\n\
cost of a query is calculated.\n\n\
To query larger amounts of data with fewer limits, bulk operations should be used instead.\n\
See https://shopify.dev/tutorials/perform-bulk-operations-with-admin-api for usage details.\n"
    );
    Error::new(message)
        .with_extension("code", "MAX_COST_EXCEEDED")
        .with_extension("cost", requested)
        .with_extension("maxCost", MAX_COST)
        .with_extension(
            "documentation",
            "https://shopify.dev/api/usage/limits#rate-limits",
        )
}

/// The `extensions.cost` of an answer: `actualQueryCost` is `null` when the query did not run.
pub fn extension(requested: u64, actual: Option<u64>, throttle_status: Value) -> Value {
    json!({
        "requestedQueryCost": requested,
        "actualQueryCost": actual,
        "throttleStatus": throttle_status,
    })
}
