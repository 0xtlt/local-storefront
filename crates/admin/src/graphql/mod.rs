//! A GraphQL server over a [`Resolver`]: parsing, validation, variables, execution,
//! introspection and cost, with the errors Shopify's Admin API answers (it runs graphql-ruby, whose
//! messages and `extensions.code` these are). It knows nothing of Shopify's data: a resolver
//! answers each field, the schema says what the fields are.
//!
//! A request goes through [`prepare`] (parse, select the operation, validate, coerce the
//! variables), then [`Prepared::requested_cost`] and [`Prepared::execute`]; [`run`] does all of it
//! and the cost limits, and gives the answer:
//!
//! ```text
//! {"errors": [...], "data": {...}, "extensions": {"cost": {...}}}
//! ```
//!
//! `errors` comes first, as graphql-ruby writes it, and only when there are errors; `data` is
//! missing when the request did not run (a parse, validation or variable error, a query over the
//! cost limit, a throttled request), `null` when a `null` reached the root.
//!
//! # Writing a resolver
//!
//! The executor walks the query and asks the resolver for one field of one object at a time,
//! with a [`FieldContext`]: the field's definition and type, its arguments (coerced to JSON, with
//! the defaults of the schema), and where it is. The resolver answers an [`Output`]: a scalar or
//! an enum value as JSON, an object (an opaque handle the resolver gives back to itself for the
//! fields below it), a list of them, or `null`. The executor completes the value against the type
//! (a `null` in a non-null field goes up to the nearest nullable one, with graphql-ruby's error),
//! resolves the selected fields of each object, answers `__typename` and introspection itself,
//! and adds up the cost.
//!
//! ```
//! use lsf_admin::graphql::{self, FieldContext, FieldError, Output, Request, Resolver};
//! use serde_json::json;
//!
//! struct Shop;
//!
//! impl Resolver for Shop {
//!     type Object = &'static str; // the type of the object, here
//!
//!     fn query_root(&mut self) -> &'static str { "QueryRoot" }
//!     fn mutation_root(&mut self) -> &'static str { "Mutation" }
//!     fn type_name(&self, object: &&'static str) -> String { object.to_string() }
//!
//!     fn field(&mut self, ctx: &FieldContext<'_>, object: &&'static str)
//!         -> Result<Output<&'static str>, FieldError> {
//!         match (*object, ctx.field_name()) {
//!             ("QueryRoot", "shop") => Ok(Output::Object("Shop")),
//!             ("Shop", "name") => Ok(Output::Value(json!("My shop"))),
//!             _ => Ok(Output::Null),
//!         }
//!     }
//! }
//!
//! let schema = lsf_admin::schema::admin();
//! let answer = graphql::run(schema, &mut Shop, &Request::new("{ shop { name } }"), None);
//! assert_eq!(answer.to_json()["data"], json!({"shop": {"name": "My shop"}}));
//! ```
//!
//! A resolver only answers the fields it is asked: what a field is (a connection, a list, an
//! enum) is in the schema ([`FieldContext::ty`], [`crate::schema::Schema`]); what the query selects
//! below it is in [`FieldContext::selection`], for a resolver that loads less when less is asked.

pub mod ast;
pub mod cost;
mod error;
mod execute;
mod input;
mod introspection;
pub mod parse;
pub(crate) mod print;
mod validate;

use std::collections::HashMap;

use serde_json::{Map, Value};

pub use ast::{OperationKind, Pos};
pub use error::Error;

use crate::schema::{Field as FieldDef, Schema, TypeRef};
use crate::throttle::{self, Throttle};
use execute::Context;

/// What a [`Resolver`] answers for a field.
#[derive(Clone, Debug, PartialEq)]
pub enum Output<O> {
    /// `null`.
    Null,
    /// A scalar or an enum value (an enum value is its name), or a list of them.
    Value(Value),
    /// An object, which the executor asks the resolver the selected fields of.
    Object(O),
    /// A list of objects (or of values, or of lists).
    List(Vec<Output<O>>),
}

impl<O> Output<O> {
    /// A scalar or enum value.
    pub fn value(value: impl Into<Value>) -> Output<O> {
        Output::Value(value.into())
    }

    /// A list of objects.
    pub fn objects(objects: impl IntoIterator<Item = O>) -> Output<O> {
        Output::List(objects.into_iter().map(Output::Object).collect())
    }

    /// An object, or `null`.
    pub fn object_or_null(object: Option<O>) -> Output<O> {
        object.map_or(Output::Null, Output::Object)
    }
}

/// An error a resolver answers for a field: the field is `null` (or, when its type is non-null,
/// the nearest nullable field above it), and the response has the error with the field's
/// location and path.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldError {
    pub message: String,
    /// The `extensions` of the error: `{"code": "ACCESS_DENIED", ...}`.
    pub extensions: Option<Map<String, Value>>,
}

impl FieldError {
    pub fn new(message: impl Into<String>) -> FieldError {
        FieldError {
            message: message.into(),
            extensions: None,
        }
    }

    /// Sets `extensions.code`.
    pub fn with_code(self, code: &str) -> FieldError {
        self.with_extension("code", code)
    }

    pub fn with_extension(mut self, key: &str, value: impl Into<Value>) -> FieldError {
        self.extensions
            .get_or_insert_with(Map::new)
            .insert(key.to_string(), value.into());
        self
    }
}

/// What answers the fields of a query.
///
/// `Object` is whatever the resolver needs to find an object again: a GID, an index, an
/// enum of the kinds of objects it serves. The executor only clones it and hands it back.
pub trait Resolver {
    type Object: Clone;

    /// The object the fields of a query are asked of.
    fn query_root(&mut self) -> Self::Object;

    /// The object the fields of a mutation are asked of. They are asked one after the other,
    /// in the order of the document, each completed before the next.
    fn mutation_root(&mut self) -> Self::Object;

    /// The object type of an object: what `__typename` answers and fragments are matched
    /// against. The executor asks only for the objects of an interface or a union field; the
    /// type of an object field is the field's type.
    fn type_name(&self, object: &Self::Object) -> String;

    /// The value of a field of an object.
    fn field(
        &mut self,
        ctx: &FieldContext<'_>,
        object: &Self::Object,
    ) -> Result<Output<Self::Object>, FieldError>;
}

/// What a resolver knows of the field it is asked.
pub struct FieldContext<'a> {
    context: &'a Context<'a>,
    parent_type: &'a str,
    field: &'a FieldDef,
    /// The nodes of the field in the query: one, or more when the query selects it twice
    /// (`{ a { x } a { y } }`), which execute as one field.
    nodes: &'a [&'a ast::Field<'a>],
    arguments: &'a Map<String, Value>,
    path: &'a [Value],
}

impl<'a> FieldContext<'a> {
    /// The schema the query runs against.
    pub fn schema(&self) -> &'a Schema {
        self.context.schema
    }

    /// The object type the field is asked of: `Product` (never an interface).
    pub fn parent_type(&self) -> &'a str {
        self.parent_type
    }

    /// The name of the field: `title`.
    pub fn field_name(&self) -> &'a str {
        &self.field.name
    }

    /// The key of the field in the response: its alias, or its name.
    pub fn response_key(&self) -> &'a str {
        self.nodes[0].response_key()
    }

    /// The definition of the field in the schema: its type, arguments, description.
    pub fn definition(&self) -> &'a FieldDef {
        self.field
    }

    /// The type of the field: `ProductConnection!`.
    pub fn ty(&self) -> &'a TypeRef {
        &self.field.ty
    }

    /// The arguments, coerced to JSON as the resolver reads them: those the query gives, with
    /// the default of the schema for those it leaves out (an argument given a variable without a
    /// value is left out); enum values as their names; a value given for a list as a list of
    /// one; `Decimal`, `Money`, `BigInt` and `UnsignedInt64` as strings; the defaults of input
    /// objects filled in. An argument with neither a value nor a default is not there.
    pub fn arguments(&self) -> &'a Map<String, Value> {
        self.arguments
    }

    /// The same field with other arguments: what a version's adapter of the arguments gives the
    /// resolver (`crate::version::Adapter::Arguments`), rewritten into the shape it reads.
    pub fn with_arguments<'b>(&'b self, arguments: &'b Map<String, Value>) -> FieldContext<'b>
    where
        'a: 'b,
    {
        FieldContext {
            context: self.context,
            parent_type: self.parent_type,
            field: self.field,
            nodes: self.nodes,
            arguments,
            path: self.path,
        }
    }

    /// An argument, `None` when it is missing or `null`.
    pub fn arg(&self, name: &str) -> Option<&'a Value> {
        self.arguments.get(name).filter(|value| !value.is_null())
    }

    /// A string argument (an `ID`, a `String`, an enum value).
    pub fn arg_str(&self, name: &str) -> Option<&'a str> {
        self.arg(name)?.as_str()
    }

    pub fn arg_i64(&self, name: &str) -> Option<i64> {
        self.arg(name)?.as_i64()
    }

    pub fn arg_bool(&self, name: &str) -> Option<bool> {
        self.arg(name)?.as_bool()
    }

    /// The path of the field in the response: `["products", "edges", 0, "node", "title"]`.
    pub fn path(&self) -> &'a [Value] {
        self.path
    }

    /// The path as one string: `products.edges.0.node.title`.
    pub fn path_string(&self) -> String {
        let segments: Vec<String> = self
            .path
            .iter()
            .map(|segment| match segment {
                Value::String(key) => key.clone(),
                other => other.to_string(),
            })
            .collect();
        segments.join(".")
    }

    /// The arguments of a directive the query puts on the field, coerced (`None` when the
    /// field does not have it): `directive("idempotent")` gives `{"key": "..."}`.
    pub fn directive(&self, name: &str) -> Option<Map<String, Value>> {
        let schema = self.context.schema;
        let def = schema.directive(name)?;
        let directive = self
            .nodes
            .iter()
            .flat_map(|node| &node.directives)
            .find(|directive| directive.name == name)?;
        let mut arguments = Map::new();
        for arg in def.args.values() {
            let given = directive.arguments.iter().find(|a| a.name == arg.name);
            let value = given
                .and_then(|given| {
                    input::coerce_literal(schema, &given.value, &arg.ty, self.context.variables)
                })
                .or_else(|| arg.default_value.clone());
            if let Some(value) = value {
                arguments.insert(arg.name.clone(), value);
            }
        }
        Some(arguments)
    }

    /// What the query selects below the field.
    pub fn selection(&self) -> Lookahead<'a> {
        Lookahead {
            context: self.context,
            sets: self.nodes.iter().map(|node| &node.selection_set).collect(),
        }
    }
}

/// The fields a query selects below a field, whatever the type their fragments are on, with
/// `@skip` and `@include` applied.
pub struct Lookahead<'a> {
    context: &'a Context<'a>,
    sets: Vec<&'a ast::SelectionSet<'a>>,
}

impl<'a> Lookahead<'a> {
    fn fields(&self) -> indexmap::IndexMap<&'a str, Vec<&'a ast::Field<'a>>> {
        execute::collect_fields(self.context, None, &self.sets)
    }

    /// The names of the fields selected (not their aliases), each once, in order.
    pub fn names(&self) -> Vec<&'a str> {
        let mut names = Vec::new();
        for nodes in self.fields().values() {
            for node in nodes {
                if !names.contains(&node.name) {
                    names.push(node.name);
                }
            }
        }
        names
    }

    /// Whether a field is selected.
    pub fn selects(&self, name: &str) -> bool {
        self.fields()
            .values()
            .flatten()
            .any(|node| node.name == name)
    }

    /// What is selected below the fields of a name: `selection().field("edges").field("node")`.
    pub fn field(&self, name: &str) -> Lookahead<'a> {
        let sets = self
            .fields()
            .values()
            .flatten()
            .filter(|node| node.name == name)
            .map(|node| &node.selection_set)
            .collect();
        Lookahead {
            context: self.context,
            sets,
        }
    }

    /// Whether nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.fields().is_empty()
    }
}

/// A GraphQL request: what `POST /admin/api/<version>/graphql.json` is sent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub query: String,
    /// The values of the variables (`variables` of the request; an absent or `null` one is
    /// empty).
    pub variables: Map<String, Value>,
    /// `operationName`; an empty one is none.
    pub operation_name: Option<String>,
}

impl Request {
    pub fn new(query: impl Into<String>) -> Request {
        Request {
            query: query.into(),
            ..Request::default()
        }
    }

    pub fn with_variables(mut self, variables: Value) -> Request {
        if let Value::Object(variables) = variables {
            self.variables = variables;
        }
        self
    }

    pub fn with_operation_name(mut self, name: &str) -> Request {
        self.operation_name = Some(name.to_string());
        self
    }
}

/// The answer to a request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Response {
    pub errors: Vec<Error>,
    /// `None` when the request did not run: the answer has no `data`.
    pub data: Option<Value>,
    /// `extensions` of the answer: `cost`, and what the caller adds (`lsf`).
    pub extensions: Map<String, Value>,
}

impl Response {
    pub fn from_errors(errors: Vec<Error>) -> Response {
        Response {
            errors,
            ..Response::default()
        }
    }

    /// The JSON of the answer, its keys in Shopify's order: `errors`, `data`, `extensions`.
    pub fn to_json(&self) -> Value {
        let mut answer = Map::new();
        if !self.errors.is_empty() {
            let errors = self.errors.iter().map(Error::to_json).collect();
            answer.insert("errors".into(), Value::Array(errors));
        }
        if let Some(data) = &self.data {
            answer.insert("data".into(), data.clone());
        }
        if !self.extensions.is_empty() {
            answer.insert("extensions".into(), Value::Object(self.extensions.clone()));
        }
        Value::Object(answer)
    }
}

/// A request parsed, validated, with its operation selected and its variables coerced: ready
/// to cost and to run.
pub struct Prepared<'s, 'q> {
    schema: &'s Schema,
    document: ast::Document<'q>,
    /// The index of the operation in the document.
    operation: usize,
    variables: Map<String, Value>,
}

/// Parses a query, selects its operation, validates the document and coerces the variables,
/// as graphql-ruby does in that order: a parse error, an operation that cannot be selected,
/// validation errors, then variable errors, each stopping there.
pub fn prepare<'s, 'q>(
    schema: &'s Schema,
    query: &'q str,
    operation_name: Option<&str>,
    variables: &Map<String, Value>,
) -> Result<Prepared<'s, 'q>, Vec<Error>> {
    let document = parse::parse(query).map_err(|error| vec![*error])?;
    let operation_name = operation_name.filter(|name| !name.is_empty());
    let operations: Vec<(usize, &ast::Operation<'_>)> = document
        .definitions
        .iter()
        .enumerate()
        .filter_map(|(index, definition)| match definition {
            ast::Definition::Operation(operation) => Some((index, operation)),
            _ => None,
        })
        .collect();
    let selected = match (operation_name, operations.as_slice()) {
        (None, [(index, _)]) => Some(*index),
        (name, _) => operations
            .iter()
            .find(|(_, operation)| operation.name == name)
            .map(|(index, _)| *index),
    };
    let Some(operation) = selected else {
        if operations.is_empty() {
            // A document of fragments only: validation says what is wrong with it.
            let errors = validate::validate(schema, &document);
            return Err(errors);
        }
        let message = match operation_name {
            Some(name) => format!("No operation named \"{name}\""),
            None => "An operation name is required".to_string(),
        };
        return Err(vec![Error::new(message)]);
    };
    let errors = validate::validate(schema, &document);
    if !errors.is_empty() {
        return Err(errors);
    }
    let ast::Definition::Operation(definition) = &document.definitions[operation] else {
        return Err(vec![Error::new("An operation name is required")]);
    };
    if nests_too_deep(&document, definition) {
        return Err(vec![Error::new("This query is too large to execute.")]);
    }
    let variables = coerce_variables(schema, definition, variables)?;
    Ok(Prepared {
        schema,
        document,
        operation,
        variables,
    })
}

/// The values of the variables of an operation, checked and coerced as graphql-ruby does: an
/// error per variable whose value does not fit (`Variable $id of type ID! was provided invalid
/// value`), with the value and the problems in `extensions`.
fn coerce_variables(
    schema: &Schema,
    operation: &ast::Operation<'_>,
    provided: &Map<String, Value>,
) -> Result<Map<String, Value>, Vec<Error>> {
    let mut values = Map::new();
    let mut errors = Vec::new();
    for definition in &operation.variables {
        if !schema
            .get(definition.ty.named())
            .is_some_and(|ty| ty.is_input())
        {
            continue;
        }
        let name = definition.name;
        let given = provided.get(name);
        let problems = match (given, &definition.default_value) {
            (Some(value), _) => input::validate_value(schema, value, &definition.ty),
            (None, None) => input::validate_value(schema, &Value::Null, &definition.ty),
            (None, Some(_)) => Vec::new(),
        };
        if problems.is_empty() {
            if let Some(value) = given {
                values.insert(
                    name.to_string(),
                    input::coerce_value(schema, value, &definition.ty),
                );
            } else if let Some(default) = &definition.default_value {
                let none = Map::new();
                if let Some(value) = input::coerce_literal(schema, default, &definition.ty, &none) {
                    values.insert(name.to_string(), value);
                }
            }
            continue;
        }
        let mut message = format!(
            "Variable ${name} of type {} was provided invalid value",
            definition.ty
        );
        let located: Vec<String> = problems
            .iter()
            .filter(|problem| !problem.path.is_empty())
            .map(|problem| {
                let path: Vec<String> = problem
                    .path
                    .iter()
                    .map(|segment| match segment {
                        Value::String(key) => key.clone(),
                        other => other.to_string(),
                    })
                    .collect();
                format!("{} ({})", path.join("."), problem.explanation)
            })
            .collect();
        if !located.is_empty() {
            message.push_str(" for ");
            message.push_str(&located.join(", "));
        }
        let problems: Vec<Value> = problems.iter().map(input::Problem::to_json).collect();
        errors.push(
            Error::new(message)
                .with_locations(vec![definition.pos])
                .with_extension("value", given.cloned().unwrap_or(Value::Null))
                .with_extension("problems", problems),
        );
    }
    if errors.is_empty() {
        Ok(values)
    } else {
        Err(errors)
    }
}

/// Whether the operation nests deeper than [`parse::MAX_DEPTH`] once its fragments are spread:
/// a fragment's selections sit as deep as its spread, which the parser does not see. Refused as
/// the parser refuses a query nested too deeply, rather than run with the deepest selections left
/// out. Called after validation, which refuses fragment cycles.
fn nests_too_deep(document: &ast::Document<'_>, operation: &ast::Operation<'_>) -> bool {
    let fragments: HashMap<&str, &ast::Fragment<'_>> = document
        .fragments()
        .map(|fragment| (fragment.name, fragment))
        .collect();
    too_deep(&operation.selection_set, 0, &fragments, &mut HashMap::new())
}

/// Whether `set`, at `depth`, nests too deeply. `fine_from` keeps the deepest depth each fragment
/// was found fine at, which a spread as deep or shallower need not check again: each fragment is
/// walked at most [`parse::MAX_DEPTH`] times, however often it is spread.
fn too_deep<'q>(
    set: &ast::SelectionSet<'q>,
    depth: usize,
    fragments: &HashMap<&'q str, &ast::Fragment<'q>>,
    fine_from: &mut HashMap<&'q str, usize>,
) -> bool {
    if depth > parse::MAX_DEPTH {
        return true;
    }
    set.items.iter().any(|selection| match selection {
        ast::Selection::Field(field) => {
            too_deep(&field.selection_set, depth + 1, fragments, fine_from)
        }
        ast::Selection::InlineFragment(inline) => {
            too_deep(&inline.selection_set, depth + 1, fragments, fine_from)
        }
        ast::Selection::FragmentSpread(spread) => {
            if fine_from
                .get(spread.name)
                .is_some_and(|&fine| fine >= depth)
            {
                return false;
            }
            let Some(fragment) = fragments.get(spread.name) else {
                return false;
            };
            if too_deep(&fragment.selection_set, depth + 1, fragments, fine_from) {
                return true;
            }
            fine_from.insert(spread.name, depth);
            false
        }
    })
}

/// What running an operation gave.
#[derive(Clone, Debug, PartialEq)]
pub struct Executed {
    /// The data: an object, or `null` when a `null` reached the root.
    pub data: Value,
    pub errors: Vec<Error>,
    /// The actual cost: what was returned.
    pub actual_cost: u64,
}

impl<'s, 'q> Prepared<'s, 'q> {
    fn operation(&self) -> &ast::Operation<'q> {
        match &self.document.definitions[self.operation] {
            ast::Definition::Operation(operation) => operation,
            _ => unreachable!("the selected definition is an operation"),
        }
    }

    fn context(&self) -> Context<'_> {
        let fragments: HashMap<&str, &ast::Fragment<'_>> = self
            .document
            .fragments()
            .map(|fragment| (fragment.name, fragment))
            .collect();
        Context {
            schema: self.schema,
            fragments,
            variables: &self.variables,
        }
    }

    /// Whether the operation is a query or a mutation.
    pub fn kind(&self) -> OperationKind {
        self.operation().kind
    }

    /// The name of the operation, if it has one.
    pub fn operation_name(&self) -> Option<&'q str> {
        self.operation().name
    }

    /// The values of the variables, coerced.
    pub fn variables(&self) -> &Map<String, Value> {
        &self.variables
    }

    /// The requested cost of the operation (see [`cost`]).
    pub fn requested_cost(&self) -> u64 {
        cost::requested(&self.context(), self.operation())
    }

    /// Runs the operation.
    pub fn execute<R: Resolver>(&self, resolver: &mut R) -> Executed {
        let context = self.context();
        let executed = execute::Executor::run(&context, self.operation(), resolver);
        Executed {
            data: executed.data,
            errors: executed.errors,
            actual_cost: executed.actual_cost,
        }
    }
}

/// Where a request's cost is taken from: the throttle, and the bucket of the shop and the app.
#[derive(Clone, Copy)]
pub struct Bucket<'a> {
    pub throttle: &'a Throttle,
    /// The key of the bucket: [`throttle::key`] of the shop and the app.
    pub key: &'a str,
}

/// Runs a request as the Admin API does: prepares it, refuses it over the single query cost
/// limit (`MAX_COST_EXCEEDED`) or when the bucket does not have its requested cost
/// (`THROTTLED`), runs it, refunds the bucket what it did not cost, and answers with
/// `extensions.cost`. Without a bucket, the cost is reported against a full bucket of the
/// default plan, and nothing is throttled.
///
/// A request that does not prepare (parse, validation or variable errors) answers only its
/// errors, as graphql-ruby does: it has no cost, since it does not run.
pub fn run<R: Resolver>(
    schema: &Schema,
    resolver: &mut R,
    request: &Request,
    bucket: Option<Bucket<'_>>,
) -> Response {
    let prepared = match prepare(
        schema,
        &request.query,
        request.operation_name.as_deref(),
        &request.variables,
    ) {
        Ok(prepared) => prepared,
        Err(errors) => return Response::from_errors(errors),
    };
    let requested = prepared.requested_cost();
    let status = |bucket: Option<Bucket<'_>>| match bucket {
        Some(bucket) => bucket.throttle.status(bucket.key),
        None => throttle::Plan::default().full(),
    };
    let mut response = Response::default();
    if requested > cost::MAX_COST {
        response.errors.push(cost::max_cost_exceeded(requested));
        let cost = cost::extension(requested, None, status(bucket).to_json());
        response.extensions.insert("cost".into(), cost);
        return response;
    }
    if let Some(bucket) = bucket
        && let Err(status) = bucket.throttle.admit(bucket.key, requested)
    {
        response.errors.push(throttle::throttled());
        let cost = cost::extension(requested, None, status.to_json());
        response.extensions.insert("cost".into(), cost);
        return response;
    }
    let executed = prepared.execute(resolver);
    let status = match bucket {
        Some(bucket) => bucket
            .throttle
            .settle(bucket.key, requested, executed.actual_cost),
        None => status(None),
    };
    response.errors = executed.errors;
    response.data = Some(executed.data);
    let cost = cost::extension(requested, Some(executed.actual_cost), status.to_json());
    response.extensions.insert("cost".into(), cost);
    response
}
