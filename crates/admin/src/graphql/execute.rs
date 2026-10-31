//! Execution: the fields of the operation, resolved by a [`Resolver`] and completed against
//! their type as the spec says (section 6): fragments that apply to the object's type, aliases,
//! `@skip` and `@include`, `__typename`, lists, and a `null` where the schema forbids one
//! propagated to the nearest nullable field, with graphql-ruby's error
//! (`Cannot return null for non-nullable field Product.title`). The root fields of a mutation run
//! one after the other, in the order of the document.
//!
//! As it completes values, it adds up their actual cost (see [`super::cost`]).

use std::collections::HashMap;

use indexmap::IndexMap;
use serde_json::{Map, Value};

use super::ast::{Directive, Field, Fragment, Operation, OperationKind, Selection, SelectionSet};
use super::cost::{self, Shape};
use super::error::Error;
use super::input;
use super::introspection::{Introspection, Meta};
use super::parse::MAX_DEPTH;
use super::{FieldContext, Output, Resolver};
use crate::schema::{Field as FieldDef, Kind, Schema, TypeDef, TypeRef};

/// What execution reads besides the resolver: the schema, the fragments of the document and
/// the values of the variables, coerced.
pub(crate) struct Context<'a> {
    pub schema: &'a Schema,
    pub fragments: HashMap<&'a str, &'a Fragment<'a>>,
    pub variables: &'a Map<String, Value>,
}

/// The fields of selection sets for an object of a type, by response key, in order: fields
/// whose `@skip`/`@include` leave them out, and fragments that do not apply to the type, are not
/// there. `None` as the type takes every fragment (a lookahead does not know the type).
pub(crate) fn collect_fields<'a>(
    context: &Context<'a>,
    object_type: Option<&str>,
    sets: &[&'a SelectionSet<'a>],
) -> IndexMap<&'a str, Vec<&'a Field<'a>>> {
    let mut fields = IndexMap::new();
    let mut visited = Vec::new();
    for set in sets {
        collect(context, object_type, set, &mut fields, &mut visited, 0);
    }
    fields
}

fn collect<'a>(
    context: &Context<'a>,
    object_type: Option<&str>,
    set: &'a SelectionSet<'a>,
    fields: &mut IndexMap<&'a str, Vec<&'a Field<'a>>>,
    visited: &mut Vec<&'a str>,
    depth: usize,
) {
    // Fragments in fragments nest deeper than the parser sees: past its limit, the rest is
    // left out (and the executor refuses the query).
    if depth > MAX_DEPTH {
        return;
    }
    let applies = |condition: &str| {
        object_type.is_none_or(|object| context.schema.is_possible(condition, object))
    };
    for selection in &set.items {
        match selection {
            Selection::Field(field) => {
                if included(context, &field.directives) {
                    fields.entry(field.response_key()).or_default().push(field);
                }
            }
            Selection::InlineFragment(inline) => {
                if included(context, &inline.directives)
                    && inline.type_condition.is_none_or(applies)
                {
                    collect(
                        context,
                        object_type,
                        &inline.selection_set,
                        fields,
                        visited,
                        depth + 1,
                    );
                }
            }
            Selection::FragmentSpread(spread) => {
                if visited.contains(&spread.name) || !included(context, &spread.directives) {
                    continue;
                }
                visited.push(spread.name);
                if let Some(fragment) = context.fragments.get(spread.name)
                    && applies(fragment.type_condition)
                {
                    collect(
                        context,
                        object_type,
                        &fragment.selection_set,
                        fields,
                        visited,
                        depth + 1,
                    );
                }
            }
        }
    }
}

/// Whether `@skip` and `@include` keep a selection.
fn included(context: &Context<'_>, directives: &[Directive<'_>]) -> bool {
    let condition = |name: &str| {
        let directive = directives.iter().find(|d| d.name == name)?;
        let argument = directive.arguments.iter().find(|a| a.name == "if")?;
        let ty = TypeRef::Named("Boolean".to_string());
        input::coerce_literal(context.schema, &argument.value, &ty, context.variables)?.as_bool()
    };
    condition("skip") != Some(true) && condition("include") != Some(false)
}

/// The arguments of a field, coerced, with the defaults of the schema for those left out.
pub(crate) fn arguments(
    context: &Context<'_>,
    def: &FieldDef,
    field: &Field<'_>,
) -> Map<String, Value> {
    let mut arguments = Map::new();
    for arg in def.args.values() {
        let given = field.argument(&arg.name).and_then(|given| {
            input::coerce_literal(context.schema, &given.value, &arg.ty, context.variables)
        });
        if let Some(value) = given.or_else(|| arg.default_value.clone()) {
            arguments.insert(arg.name.clone(), value);
        }
    }
    arguments
}

/// The result of an execution.
pub(crate) struct Executed {
    pub data: Value,
    pub errors: Vec<Error>,
    pub actual_cost: u64,
}

/// `Err` is a `null` going up to the nearest nullable position.
type Completed = Result<(Value, u64), ()>;

pub(crate) struct Executor<'c, 'a> {
    context: &'c Context<'a>,
    errors: Vec<Error>,
    /// The path of the value being completed, in the response.
    path: Vec<Value>,
    /// Whether the query nested too deeply, through its fragments, to run.
    too_large: bool,
}

impl<'c, 'a> Executor<'c, 'a> {
    pub fn run<R: Resolver>(
        context: &'c Context<'a>,
        operation: &'a Operation<'a>,
        resolver: &mut R,
    ) -> Executed {
        let mut executor = Executor {
            context,
            errors: Vec::new(),
            path: Vec::new(),
            too_large: false,
        };
        let schema = context.schema;
        let (root, root_type) = match operation.kind {
            OperationKind::Mutation => (resolver.mutation_root(), schema.mutation_type()),
            _ => (resolver.query_root(), Some(schema.query_type())),
        };
        let Some(root_type) = root_type.and_then(|name| schema.get(name)) else {
            return Executed {
                data: Value::Null,
                errors: vec![Error::new("Schema is not configured for this operation")],
                actual_cost: 0,
            };
        };
        let mutation = operation.kind == OperationKind::Mutation;
        let result = executor.selection(
            resolver,
            &root,
            root_type,
            &[&operation.selection_set],
            mutation,
        );
        let (data, actual_cost) = result.unwrap_or((Value::Null, 0));
        if executor.too_large {
            // graphql-ruby's answer when its stack runs out: the error alone.
            return Executed {
                data: Value::Null,
                errors: vec![Error::new("This query is too large to execute.")],
                actual_cost,
            };
        }
        Executed {
            data,
            errors: executor.errors,
            actual_cost,
        }
    }

    /// The fields of an object: a JSON object, and its cost.
    fn selection<R: Resolver>(
        &mut self,
        resolver: &mut R,
        object: &R::Object,
        object_type: &'a TypeDef,
        sets: &[&'a SelectionSet<'a>],
        mutation_root: bool,
    ) -> Completed {
        // A field and a list index per level, at most.
        if self.path.len() > 2 * MAX_DEPTH {
            self.too_large = true;
            return Ok((Value::Null, 0));
        }
        let fields = collect_fields(self.context, Some(&object_type.name), sets);
        let mut result = Map::new();
        let mut cost = 0;
        for (key, nodes) in fields {
            self.path.push(key.into());
            let completed = self.field(resolver, object, object_type, &nodes, mutation_root);
            self.path.pop();
            let (value, field_cost) = completed?;
            cost += field_cost;
            result.insert(key.to_string(), value);
        }
        Ok((Value::Object(result), cost))
    }

    fn field<R: Resolver>(
        &mut self,
        resolver: &mut R,
        object: &R::Object,
        object_type: &'a TypeDef,
        nodes: &[&'a Field<'a>],
        mutation_root: bool,
    ) -> Completed {
        let schema = self.context.schema;
        let node = nodes[0];
        if node.name == "__typename" {
            return Ok((Value::String(object_type.name.clone()), 0));
        }
        let Some(def) = schema.lookup_field(&object_type.name, node.name) else {
            // Validation refuses an unknown field; a resolver cannot answer one.
            return Ok((Value::Null, 0));
        };
        let arguments = arguments(self.context, def, node);
        if object_type.name == schema.query_type() && node.name.starts_with("__") {
            let mut introspection = Introspection { schema };
            let meta = match node.name {
                "__schema" => Output::Object(Meta::Schema),
                _ => match arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .and_then(|n| schema.get(n))
                {
                    Some(def) => Output::Object(Meta::Named(def)),
                    None => Output::Null,
                },
            };
            let shape = Shape::free();
            return self.complete(
                &mut introspection,
                object_type,
                def,
                nodes,
                &def.ty,
                meta,
                shape,
            );
        }
        let ctx = FieldContext {
            context: self.context,
            parent_type: &object_type.name,
            field: def,
            nodes,
            arguments: &arguments,
            path: &self.path,
        };
        let shape = cost::shape(schema, &object_type.name, def, mutation_root);
        let output = match resolver.field(&ctx, object) {
            Ok(output) => output,
            Err(error) => {
                self.errors.push(Error {
                    message: error.message,
                    locations: Some(vec![node.pos]),
                    path: Some(self.path.clone()),
                    extensions: error.extensions,
                });
                // The error stands for the null: a non-null field adds no other error.
                let cost = shape.fixed;
                return if def.ty.is_non_null() {
                    Err(())
                } else {
                    Ok((Value::Null, cost))
                };
            }
        };
        let (value, cost) =
            self.complete(resolver, object_type, def, nodes, &def.ty, output, shape)?;
        Ok((value, cost + shape.fixed))
    }

    /// Completes a value of a type.
    #[allow(clippy::too_many_arguments)]
    fn complete<R: Resolver>(
        &mut self,
        resolver: &mut R,
        parent: &'a TypeDef,
        def: &'a FieldDef,
        nodes: &[&'a Field<'a>],
        ty: &'a TypeRef,
        output: Output<R::Object>,
        shape: Shape,
    ) -> Completed {
        match ty {
            TypeRef::NonNull(inner) => {
                match self.complete_nullable(resolver, parent, def, nodes, inner, output, shape)? {
                    (Value::Null, _) => {
                        let message = if std::ptr::eq(ty, &def.ty) {
                            format!(
                                "Cannot return null for non-nullable field {}.{}",
                                parent.name, def.name
                            )
                        } else {
                            format!(
                                "Cannot return null for non-nullable element of type '{ty}' for {}.{}",
                                parent.name, def.name
                            )
                        };
                        self.errors.push(
                            Error::new(message)
                                .with_locations(vec![nodes[0].pos])
                                .with_path(self.path.clone()),
                        );
                        Err(())
                    }
                    completed => Ok(completed),
                }
            }
            _ => Ok(self
                .complete_nullable(resolver, parent, def, nodes, ty, output, shape)
                .unwrap_or((Value::Null, 0))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn complete_nullable<R: Resolver>(
        &mut self,
        resolver: &mut R,
        parent: &'a TypeDef,
        def: &'a FieldDef,
        nodes: &[&'a Field<'a>],
        ty: &'a TypeRef,
        output: Output<R::Object>,
        shape: Shape,
    ) -> Completed {
        let schema = self.context.schema;
        if matches!(output, Output::Null | Output::Value(Value::Null)) {
            return Ok((Value::Null, 0));
        }
        if let TypeRef::List(item) = ty {
            let items = match output {
                Output::List(items) => items,
                Output::Value(Value::Array(values)) => {
                    values.into_iter().map(Output::Value).collect()
                }
                _ => return self.resolver_bug(parent, def, nodes, "a single value for a list"),
            };
            let mut values = Vec::with_capacity(items.len());
            let mut cost = 0;
            for (index, item_output) in items.into_iter().enumerate() {
                self.path.push(index.into());
                let completed =
                    self.complete(resolver, parent, def, nodes, item, item_output, shape);
                self.path.pop();
                let (value, item_cost) = completed?;
                cost = if shape.sum_items {
                    cost + item_cost
                } else {
                    cost.max(item_cost)
                };
                values.push(value);
            }
            return Ok((Value::Array(values), cost));
        }
        let Some(named) = schema.get(ty.named()) else {
            return Ok((Value::Null, 0));
        };
        if named.is_leaf() {
            return match output {
                Output::Value(value) => self.leaf(parent, def, nodes, named, value),
                Output::List(_) => {
                    self.resolver_bug(parent, def, nodes, "a list for a single value")
                }
                _ => self.resolver_bug(parent, def, nodes, "an object for a scalar or an enum"),
            };
        }
        let object = match output {
            Output::Object(object) => object,
            Output::List(_) => {
                return self.resolver_bug(parent, def, nodes, "a list for a single object");
            }
            _ => return self.resolver_bug(parent, def, nodes, "a value for an object"),
        };
        let runtime = if named.kind == Kind::Object {
            named
        } else {
            let name = resolver.type_name(&object);
            match schema
                .get(&name)
                .filter(|t| t.kind == Kind::Object && schema.is_possible(&named.name, &t.name))
            {
                Some(runtime) => runtime,
                None => {
                    return self.resolver_bug(
                        parent,
                        def,
                        nodes,
                        &format!(
                            "an object of type {name}, which is not a possible type of {}",
                            named.name
                        ),
                    );
                }
            }
        };
        let sets: Vec<&'a SelectionSet<'a>> =
            nodes.iter().map(|node| &node.selection_set).collect();
        let (value, children) = self.selection(resolver, &object, runtime, &sets, false)?;
        Ok((
            value,
            shape.per_object + if shape.free_children { 0 } else { children },
        ))
    }

    /// A scalar or an enum value. An enum value must be one of the enum, as graphql-ruby
    /// requires (its `UnresolvedValueError`).
    fn leaf(
        &mut self,
        parent: &TypeDef,
        def: &FieldDef,
        nodes: &[&'a Field<'a>],
        ty: &TypeDef,
        value: Value,
    ) -> Completed {
        if ty.kind == Kind::Enum
            && !value
                .as_str()
                .is_some_and(|name| ty.enum_values.contains_key(name))
        {
            let path: Vec<String> = self
                .path
                .iter()
                .map(|segment| match segment {
                    Value::String(key) => key.clone(),
                    other => other.to_string(),
                })
                .collect();
            let message = format!(
                "`{}.{}` returned `{value}` at `{}`, but this isn't a valid value for `{}`. Update the field or resolver to return one of `{}`'s values instead.",
                parent.name,
                def.name,
                path.join("."),
                ty.name,
                ty.name
            );
            self.errors.push(
                Error::new(message)
                    .with_locations(vec![nodes[0].pos])
                    .with_path(self.path.clone()),
            );
            return Ok((Value::Null, 0));
        }
        Ok((value, 0))
    }

    /// A resolver answered a value that does not fit the type of its field: an error at the
    /// field, which is then `null`.
    fn resolver_bug(
        &mut self,
        parent: &TypeDef,
        def: &FieldDef,
        nodes: &[&'a Field<'a>],
        what: &str,
    ) -> Completed {
        self.errors.push(
            Error::new(format!(
                "lsf: the resolver of {}.{} returned {what} ({})",
                parent.name, def.name, def.ty
            ))
            .with_locations(vec![nodes[0].pos])
            .with_path(self.path.clone())
            .with_extension("code", "INTERNAL_SERVER_ERROR"),
        );
        Ok((Value::Null, 0))
    }
}
