//! A small GraphQL executor: runs a query document against a [`Graph`], which knows the
//! fields of each type. It handles what a Function input query uses: aliases, fragments (named
//! and inline), variables, `@skip`/`@include`, `__typename`, arguments. Every error names the
//! path of the offending field.

use std::collections::{HashMap, HashSet};

use graphql_parser::query::{
    Definition, Directive, Document, Field, FragmentDefinition, OperationDefinition, Selection,
    SelectionSet, Type, TypeCondition, Value,
};
use indexmap::IndexMap;
use serde_json::{Map, Value as Json};

/// What a field resolved to.
pub(super) enum Resolved<N> {
    Null,
    /// A scalar, an enum, or a list of them.
    Leaf(Json),
    Object(N),
    List(Vec<N>),
}

/// The types and fields the queries run against.
pub(super) trait Graph {
    type Node: Clone;

    fn root(&self) -> Self::Node;
    /// The GraphQL type of a node, as `__typename` reports it.
    fn type_name(&self, node: &Self::Node) -> &'static str;
    /// Whether a node is of the type (or interface, or union) a fragment names; `None` when the
    /// schema has no such type.
    fn applies(&self, node: &Self::Node, condition: &str) -> Option<bool>;
    /// The arguments of a field that have a default value in the schema. One the query leaves
    /// out, or gives a variable without a value, is worth its default.
    fn defaults(&self, _node: &Self::Node, _field: &str) -> Vec<(String, Json)> {
        Vec::new()
    }
    /// Resolves a field. `Ok(None)` when the type has no such field. The arguments the field
    /// reads are taken out of `args`; those left over are reported as unknown.
    fn field(
        &self,
        node: &Self::Node,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Self::Node>>, String>;
}

/// The arguments of a field, evaluated. A resolver takes the ones it knows.
#[derive(Default)]
pub(super) struct Args(IndexMap<String, Json>, Unset);

/// The arguments given a variable without a value, with the name of the variable: what a
/// missing argument says.
#[derive(Default)]
pub(super) struct Unset(IndexMap<String, String>);

impl Args {
    /// The error of an argument the field requires and was not given.
    fn missing(&self, name: &str) -> String {
        match self.1.0.get(name) {
            Some(variable) => format!(
                "missing required argument `{name}`: the variable `${variable}` it is given was not given a value"
            ),
            None => format!("missing required argument `{name}`"),
        }
    }

    pub(super) fn take(&mut self, name: &str) -> Option<Json> {
        self.0.shift_remove(name).filter(|value| !value.is_null())
    }

    pub(super) fn string(&mut self, name: &str) -> Result<Option<String>, String> {
        match self.take(name) {
            None => Ok(None),
            Some(Json::String(text)) => Ok(Some(text)),
            Some(other) => Err(format!("argument `{name}` must be a string, got {other}")),
        }
    }

    pub(super) fn required_string(&mut self, name: &str) -> Result<String, String> {
        match self.string(name)? {
            Some(text) => Ok(text),
            None => Err(self.missing(name)),
        }
    }

    /// A list of strings (or ids: numbers are read as their text). A single value counts as a
    /// list of one, as GraphQL coerces it.
    pub(super) fn string_list(&mut self, name: &str) -> Result<Vec<String>, String> {
        let Some(value) = self.take(name) else {
            return Err(self.missing(name));
        };
        let items = match value {
            Json::Array(items) => items,
            single => vec![single],
        };
        items
            .into_iter()
            .map(|item| match item {
                Json::String(text) => Ok(text),
                Json::Number(number) => Ok(number.to_string()),
                other => Err(format!("argument `{name}` must hold strings, got {other}")),
            })
            .collect()
    }

    fn leftover(&self) -> Option<&str> {
        self.0.keys().next().map(String::as_str)
    }
}

/// Runs the query of `document` against `graph`; `variables` are the values given for the
/// operation's variables.
pub(super) fn execute<'q, G: Graph>(
    graph: &G,
    document: &'q Document<'q, String>,
    variables: &Map<String, Json>,
) -> Result<Json, String> {
    let mut operations = Vec::new();
    let mut fragments = HashMap::new();
    for definition in &document.definitions {
        match definition {
            Definition::Operation(operation) => operations.push(operation),
            Definition::Fragment(fragment) => {
                if fragments.insert(fragment.name.as_str(), fragment).is_some() {
                    return Err(format!("fragment `{}` is defined twice", fragment.name));
                }
            }
        }
    }
    let operation = match operations.as_slice() {
        [] => return Err("the query has no operation".to_string()),
        [operation] => operation,
        _ => return Err("the query has several operations; a Function input has one".to_string()),
    };
    let (declared, selection) = match operation {
        OperationDefinition::SelectionSet(selection) => (&[][..], selection),
        OperationDefinition::Query(query) => {
            (query.variable_definitions.as_slice(), &query.selection_set)
        }
        OperationDefinition::Mutation(_) => {
            return Err("a Function input is a query, not a mutation".to_string());
        }
        OperationDefinition::Subscription(_) => {
            return Err("a Function input is a query, not a subscription".to_string());
        }
    };

    let mut values = IndexMap::new();
    for definition in declared {
        let name = definition.name.as_str();
        let value = variables.get(name).cloned().or_else(|| {
            definition
                .default_value
                .as_ref()
                .map(|default| literal(default, &IndexMap::new()).unwrap_or(Json::Null))
        });
        match value {
            Some(value) if !(value.is_null() && is_required(&definition.var_type)) => {
                values.insert(name.to_string(), value);
            }
            _ if is_required(&definition.var_type) => {
                return Err(format!(
                    "variable `${name}` of type `{}` is required and was not given a value",
                    definition.var_type
                ));
            }
            _ => {}
        }
    }

    check_fragment_cycles(&fragments)?;
    let executor = Executor {
        graph,
        fragments,
        variables: values,
    };
    let mut path = Vec::new();
    executor.object(&graph.root(), &[selection], &mut path)
}

fn is_required(var_type: &Type<'_, String>) -> bool {
    matches!(var_type, Type::NonNullType(_))
}

fn check_fragment_cycles(
    fragments: &HashMap<&str, &FragmentDefinition<'_, String>>,
) -> Result<(), String> {
    fn spreads<'q>(set: &'q SelectionSet<'_, String>, out: &mut Vec<&'q str>) {
        for selection in &set.items {
            match selection {
                Selection::Field(field) => spreads(&field.selection_set, out),
                Selection::FragmentSpread(spread) => out.push(spread.fragment_name.as_str()),
                Selection::InlineFragment(inline) => spreads(&inline.selection_set, out),
            }
        }
    }
    fn visit<'q>(
        name: &'q str,
        fragments: &HashMap<&'q str, &'q FragmentDefinition<'_, String>>,
        stack: &mut Vec<&'q str>,
        done: &mut HashSet<&'q str>,
    ) -> Result<(), String> {
        if stack.contains(&name) {
            return Err(format!("fragment `{name}` spreads itself"));
        }
        if !done.insert(name) {
            return Ok(());
        }
        let Some(fragment) = fragments.get(name) else {
            return Ok(());
        };
        stack.push(name);
        let mut inner = Vec::new();
        spreads(&fragment.selection_set, &mut inner);
        for spread in inner {
            visit(spread, fragments, stack, done)?;
        }
        stack.pop();
        Ok(())
    }
    let mut done = HashSet::new();
    for name in fragments.keys() {
        visit(name, fragments, &mut Vec::new(), &mut done)?;
    }
    Ok(())
}

struct Executor<'q, 'g, G: Graph> {
    graph: &'g G,
    fragments: HashMap<&'q str, &'q FragmentDefinition<'q, String>>,
    variables: IndexMap<String, Json>,
}

type Fields<'q> = IndexMap<String, Vec<&'q Field<'q, String>>>;

impl<'q, G: Graph> Executor<'q, '_, G> {
    fn fail<T>(
        &self,
        path: &[String],
        position: graphql_parser::Pos,
        message: String,
    ) -> Result<T, String> {
        let at = if path.is_empty() {
            String::new()
        } else {
            format!(" at `{}`", path.join("."))
        };
        Err(format!(
            "{message}{at} (line {}, column {})",
            position.line, position.column
        ))
    }

    /// Runs the selections on an object node: fields with the same response key are merged.
    fn object(
        &self,
        node: &G::Node,
        selections: &[&'q SelectionSet<'q, String>],
        path: &mut Vec<String>,
    ) -> Result<Json, String> {
        let mut fields = Fields::new();
        let mut seen = HashSet::new();
        for set in selections {
            self.collect(node, set, &mut fields, &mut seen, path)?;
        }
        let mut out = Map::new();
        for (key, group) in fields {
            let first = group[0];
            let name = first.name.as_str();
            path.push(key.clone());
            let value = self.field(node, name, &group, path)?;
            path.pop();
            out.insert(key, value);
        }
        Ok(Json::Object(out))
    }

    fn field(
        &self,
        node: &G::Node,
        name: &str,
        group: &[&'q Field<'q, String>],
        path: &mut Vec<String>,
    ) -> Result<Json, String> {
        let first = group[0];
        let sub: Vec<&SelectionSet<'q, String>> = group
            .iter()
            .map(|field| &field.selection_set)
            .filter(|set| !set.items.is_empty())
            .collect();
        if name == "__typename" {
            if !sub.is_empty() || !first.arguments.is_empty() {
                return self.fail(
                    path,
                    first.position,
                    "`__typename` takes no arguments or selection".to_string(),
                );
            }
            return Ok(Json::String(self.graph.type_name(node).to_string()));
        }
        let type_name = self.graph.type_name(node);
        for other in &group[1..] {
            if other.name != first.name || other.arguments != first.arguments {
                return self.fail(
                    path,
                    other.position,
                    format!(
                        "fields with the same response key conflict: `{}` and `{}`",
                        first.name, other.name
                    ),
                );
            }
        }
        let mut args = Args::default();
        for (arg, value) in &first.arguments {
            match self.value(value) {
                Ok(Some(value)) => {
                    args.0.insert(arg.clone(), value);
                }
                Ok(None) => {
                    if let Value::Variable(variable) = value {
                        args.1.0.insert(arg.clone(), variable.clone());
                    }
                }
                Err(message) => return self.fail(path, first.position, message),
            }
        }
        for (arg, default) in self.graph.defaults(node, name) {
            args.0.entry(arg).or_insert(default);
        }
        let resolved = match self.graph.field(node, name, &mut args) {
            Ok(Some(resolved)) => resolved,
            Ok(None) => {
                return self.fail(
                    path,
                    first.position,
                    format!("unknown field `{name}` on type `{type_name}`"),
                );
            }
            Err(message) => {
                return self.fail(
                    path,
                    first.position,
                    format!("field `{name}` of `{type_name}`: {message}"),
                );
            }
        };
        if let Some(extra) = args.leftover() {
            return self.fail(
                path,
                first.position,
                format!("unknown argument `{extra}` on field `{name}` of `{type_name}`"),
            );
        }
        match resolved {
            Resolved::Null => Ok(Json::Null),
            Resolved::Leaf(value) => {
                if sub.is_empty() {
                    Ok(value)
                } else {
                    self.fail(
                        path,
                        first.position,
                        format!(
                            "field `{name}` of `{type_name}` is a scalar and takes no selection"
                        ),
                    )
                }
            }
            Resolved::Object(child) => {
                if sub.is_empty() {
                    return self.fail(
                        path,
                        first.position,
                        format!("field `{name}` of `{type_name}` needs a selection of subfields"),
                    );
                }
                self.object(&child, &sub, path)
            }
            Resolved::List(children) => {
                if sub.is_empty() {
                    return self.fail(
                        path,
                        first.position,
                        format!("field `{name}` of `{type_name}` needs a selection of subfields"),
                    );
                }
                children
                    .iter()
                    .map(|child| self.object(child, &sub, path))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Json::Array)
            }
        }
    }

    fn collect(
        &self,
        node: &G::Node,
        set: &'q SelectionSet<'q, String>,
        fields: &mut Fields<'q>,
        seen: &mut HashSet<&'q str>,
        path: &[String],
    ) -> Result<(), String> {
        for selection in &set.items {
            match selection {
                Selection::Field(field) => {
                    if !self.included(&field.directives, path, field.position)? {
                        continue;
                    }
                    let key = field.alias.as_ref().unwrap_or(&field.name).clone();
                    fields.entry(key).or_default().push(field);
                }
                Selection::FragmentSpread(spread) => {
                    if !self.included(&spread.directives, path, spread.position)? {
                        continue;
                    }
                    let name = spread.fragment_name.as_str();
                    let Some(fragment) = self.fragments.get(name) else {
                        return self.fail(
                            path,
                            spread.position,
                            format!("unknown fragment `{name}`"),
                        );
                    };
                    let TypeCondition::On(condition) = &fragment.type_condition;
                    if !self.fits(node, condition, path, spread.position)? || !seen.insert(name) {
                        continue;
                    }
                    self.collect(node, &fragment.selection_set, fields, seen, path)?;
                }
                Selection::InlineFragment(inline) => {
                    if !self.included(&inline.directives, path, inline.position)? {
                        continue;
                    }
                    if let Some(TypeCondition::On(condition)) = &inline.type_condition
                        && !self.fits(node, condition, path, inline.position)?
                    {
                        continue;
                    }
                    self.collect(node, &inline.selection_set, fields, seen, path)?;
                }
            }
        }
        Ok(())
    }

    fn fits(
        &self,
        node: &G::Node,
        condition: &str,
        path: &[String],
        position: graphql_parser::Pos,
    ) -> Result<bool, String> {
        match self.graph.applies(node, condition) {
            Some(applies) => Ok(applies),
            None => self.fail(path, position, format!("unknown type `{condition}`")),
        }
    }

    /// `@skip(if:)` and `@include(if:)`.
    fn included(
        &self,
        directives: &[Directive<'q, String>],
        path: &[String],
        position: graphql_parser::Pos,
    ) -> Result<bool, String> {
        for directive in directives {
            let skip = match directive.name.as_str() {
                "skip" => true,
                "include" => false,
                _ => continue,
            };
            let condition = directive
                .arguments
                .iter()
                .find(|(name, _)| name == "if")
                .map(|(_, value)| self.value(value))
                .transpose()
                .or_else(|message| self.fail(path, position, message))?
                .flatten();
            let Some(Json::Bool(condition)) = condition else {
                return self.fail(
                    path,
                    position,
                    format!("`@{}` needs a boolean `if` argument", directive.name),
                );
            };
            if condition == skip {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `None` for a variable that was not given a value: the argument is left out.
    fn value(&self, value: &Value<'_, String>) -> Result<Option<Json>, String> {
        if let Value::Variable(name) = value {
            return match self.variables.get(name) {
                Some(value) => Ok(Some(value.clone())),
                None => Ok(None),
            };
        }
        literal(value, &self.variables).map(Some)
    }
}

/// A GraphQL value as JSON; an undefined variable inside a list or object reads as null.
fn literal(value: &Value<'_, String>, variables: &IndexMap<String, Json>) -> Result<Json, String> {
    Ok(match value {
        Value::Variable(name) => variables.get(name).cloned().unwrap_or(Json::Null),
        Value::Int(number) => number
            .as_i64()
            .map(Json::from)
            .ok_or_else(|| "integer out of range".to_string())?,
        Value::Float(number) => serde_json::Number::from_f64(*number)
            .map(Json::Number)
            .ok_or_else(|| "number out of range".to_string())?,
        Value::String(text) | Value::Enum(text) => Json::String(text.clone()),
        Value::Boolean(flag) => Json::Bool(*flag),
        Value::Null => Json::Null,
        Value::List(items) => Json::Array(
            items
                .iter()
                .map(|item| literal(item, variables))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(entries) => Json::Object(
            entries
                .iter()
                .map(|(key, item)| Ok((key.clone(), literal(item, variables)?)))
                .collect::<Result<Map<_, _>, String>>()?,
        ),
    })
}
