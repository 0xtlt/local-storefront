//! A Function API schema (GraphQL SDL) and the two checks it is used for:
//!
//! - [`Schema::validate_query`]: the input query of a Function only asks for fields that exist
//!   in the schema of its API version, with arguments the schema knows;
//! - [`Schema::validate_output`]: the output of a Function fits the input object that is the
//!   result of its target (`CartDeliveryOptionsTransformRunResult`, `FunctionRunResult`, ...):
//!   no unknown field, no missing required field, the right scalars, `@oneOf` objects with
//!   exactly one field.
//!
//! The schemas are in `crates/checkout/schemas`, one file per API and per release that changed it
//! (see [`crate::version`]).

use std::collections::{BTreeSet, HashMap};

use graphql_parser::query::{self as q, Definition, Selection, SelectionSet};
use graphql_parser::schema as s;
use serde_json::Value;

#[derive(Debug)]
pub struct Schema {
    query: String,
    types: HashMap<String, TypeDef>,
}

#[derive(Debug)]
enum TypeDef {
    Scalar,
    Enum(Vec<String>),
    Object {
        fields: HashMap<String, Field>,
        interfaces: Vec<String>,
    },
    Interface {
        fields: HashMap<String, Field>,
    },
    Union(Vec<String>),
    Input {
        fields: Vec<InputField>,
        one_of: bool,
    },
}

#[derive(Debug)]
struct Field {
    ty: TypeRef,
    args: Vec<InputField>,
}

#[derive(Debug)]
struct InputField {
    name: String,
    ty: TypeRef,
    /// The value of the `= default` of the schema, as JSON.
    default: Option<Value>,
}

impl InputField {
    fn required(&self) -> bool {
        matches!(self.ty, TypeRef::NonNull(_)) && self.default.is_none()
    }
}

#[derive(Debug, Clone)]
enum TypeRef {
    Named(String),
    List(Box<TypeRef>),
    NonNull(Box<TypeRef>),
}

impl TypeRef {
    fn from(ty: &s::Type<'_, String>) -> TypeRef {
        match ty {
            s::Type::NamedType(name) => TypeRef::Named(name.clone()),
            s::Type::ListType(inner) => TypeRef::List(Box::new(TypeRef::from(inner))),
            s::Type::NonNullType(inner) => TypeRef::NonNull(Box::new(TypeRef::from(inner))),
        }
    }

    fn base(&self) -> &str {
        match self {
            TypeRef::Named(name) => name,
            TypeRef::List(inner) | TypeRef::NonNull(inner) => inner.base(),
        }
    }
}

impl Schema {
    pub fn parse(source: &str) -> Result<Schema, String> {
        let document = s::parse_schema::<String>(source).map_err(|error| error.to_string())?;
        let mut query = "Query".to_string();
        // The built-in scalars are not declared in SDL.
        let mut types: HashMap<String, TypeDef> = ["Boolean", "Float", "Int", "String", "ID"]
            .into_iter()
            .map(|name| (name.to_string(), TypeDef::Scalar))
            .collect();
        for definition in &document.definitions {
            match definition {
                s::Definition::SchemaDefinition(schema) => {
                    if let Some(name) = &schema.query {
                        query = name.clone();
                    }
                }
                s::Definition::TypeDefinition(definition) => {
                    let (name, def) = type_def(definition);
                    types.insert(name, def);
                }
                _ => {}
            }
        }
        Ok(Schema { query, types })
    }

    /// Whether the schema has a type of that name.
    pub fn has_type(&self, name: &str) -> bool {
        self.types.contains_key(name)
    }

    /// The names of the fields of an object or input object type, sorted.
    pub fn field_names(&self, type_name: &str) -> Vec<&str> {
        let mut names: Vec<&str> = match self.types.get(type_name) {
            Some(TypeDef::Object { fields, .. } | TypeDef::Interface { fields }) => {
                fields.keys().map(String::as_str).collect()
            }
            Some(TypeDef::Input { fields, .. }) => {
                fields.iter().map(|field| field.name.as_str()).collect()
            }
            _ => Vec::new(),
        };
        names.sort_unstable();
        names
    }

    // ---- input queries ----

    /// The arguments of a field that have a default value in the schema, with that value: what
    /// an argument the query leaves out (or gives an undefined variable) is worth.
    pub fn argument_defaults(&self, type_name: &str, field: &str) -> Vec<(&str, Value)> {
        let Some(field) = self.field(type_name, field) else {
            return Vec::new();
        };
        field
            .args
            .iter()
            .filter_map(|arg| Some((arg.name.as_str(), arg.default.clone()?)))
            .collect()
    }

    /// Checks the fields, the arguments and the type conditions of an input query. The error
    /// lists every problem, each with the path of the field.
    pub fn validate_query<'q>(&self, document: &'q q::Document<'q, String>) -> Result<(), String> {
        let fragments: HashMap<&'q str, &'q q::FragmentDefinition<'q, String>> = document
            .definitions
            .iter()
            .filter_map(|definition| match definition {
                Definition::Fragment(fragment) => Some((fragment.name.as_str(), fragment)),
                _ => None,
            })
            .collect();
        let mut walk = Walk {
            schema: self,
            fragments: &fragments,
            problems: Vec::new(),
            visiting: Vec::new(),
        };
        for definition in &document.definitions {
            let (selection_set, root) = match definition {
                Definition::Operation(q::OperationDefinition::Query(query)) => {
                    (&query.selection_set, &self.query)
                }
                Definition::Operation(q::OperationDefinition::SelectionSet(set)) => {
                    (set, &self.query)
                }
                Definition::Operation(_) => {
                    walk.problems.push("only queries are allowed".to_string());
                    continue;
                }
                Definition::Fragment(_) => continue,
            };
            walk.selection_set(selection_set, root, &mut Vec::new());
        }
        // The same fragment can be spread twice.
        let mut seen = BTreeSet::new();
        let mut problems = walk.problems;
        problems.retain(|problem| seen.insert(problem.clone()));
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    fn field(&self, parent: &str, name: &str) -> Option<&Field> {
        match self.types.get(parent)? {
            TypeDef::Object { fields, .. } | TypeDef::Interface { fields } => fields.get(name),
            _ => None,
        }
    }

    /// The object types a type can be at run time.
    fn objects_of(&self, name: &str) -> BTreeSet<String> {
        match self.types.get(name) {
            Some(TypeDef::Object { .. }) => BTreeSet::from([name.to_string()]),
            Some(TypeDef::Union(members)) => members.iter().cloned().collect(),
            Some(TypeDef::Interface { .. }) => self
                .types
                .iter()
                .filter(|(_, def)| {
                    matches!(def, TypeDef::Object { interfaces, .. } if interfaces.iter().any(|i| i == name))
                })
                .map(|(object, _)| object.clone())
                .collect(),
            _ => BTreeSet::new(),
        }
    }

    // ---- outputs ----

    /// The operations a result type accepts: the fields of the input object its `operations`
    /// list holds. `None` when the result has no `operations` (the discount and validation
    /// results of the first generation of the APIs).
    pub fn operations(&self, result: &str) -> Option<Vec<&str>> {
        let Some(TypeDef::Input { fields, .. }) = self.types.get(result) else {
            return None;
        };
        let operations = fields.iter().find(|field| field.name == "operations")?;
        Some(self.field_names(operations.ty.base()))
    }

    /// Checks the output of a Function against the input object `result` of the schema.
    pub fn validate_output(&self, result: &str, output: &Value) -> Result<(), String> {
        let mut problems = Vec::new();
        self.check_value(
            &TypeRef::NonNull(Box::new(TypeRef::Named(result.to_string()))),
            output,
            &mut String::new(),
            &mut problems,
        );
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    /// Gives the output the shape GraphQL input coercion would: a single value where the schema
    /// has a list becomes a list of one, at any depth. Validation accepts both forms, readers
    /// of the output only want the lists. Call it on an output that [`validate_output`]
    /// accepted.
    ///
    /// [`validate_output`]: Schema::validate_output
    pub fn normalize_output(&self, result: &str, output: &mut Value) {
        self.coerce(&TypeRef::Named(result.to_string()), output);
    }

    fn coerce(&self, ty: &TypeRef, value: &mut Value) {
        match ty {
            TypeRef::NonNull(inner) => self.coerce(inner, value),
            _ if value.is_null() => {}
            TypeRef::List(inner) => {
                if !value.is_array() {
                    *value = Value::Array(vec![value.take()]);
                }
                if let Some(items) = value.as_array_mut() {
                    for item in items {
                        self.coerce(inner, item);
                    }
                }
            }
            TypeRef::Named(name) => {
                if let Some(TypeDef::Input { fields, .. }) = self.types.get(name)
                    && let Some(object) = value.as_object_mut()
                {
                    for field in fields {
                        if let Some(value) = object.get_mut(&field.name) {
                            self.coerce(&field.ty, value);
                        }
                    }
                }
            }
        }
    }

    fn check_value(
        &self,
        ty: &TypeRef,
        value: &Value,
        path: &mut String,
        problems: &mut Vec<String>,
    ) {
        let here = |path: &str| {
            if path.is_empty() {
                "the output".to_string()
            } else {
                format!("`{path}`")
            }
        };
        match ty {
            TypeRef::NonNull(inner) => {
                if value.is_null() {
                    problems.push(format!("{} is null but cannot be", here(path)));
                } else {
                    self.check_value(inner, value, path, problems);
                }
            }
            _ if value.is_null() => {}
            TypeRef::List(inner) => match value.as_array() {
                Some(items) => {
                    for (index, item) in items.iter().enumerate() {
                        let length = path.len();
                        path.push_str(&format!("[{index}]"));
                        self.check_value(inner, item, path, problems);
                        path.truncate(length);
                    }
                }
                // GraphQL coerces a single value into a list of one.
                None => self.check_value(inner, value, path, problems),
            },
            TypeRef::Named(name) => self.check_named(name, value, path, problems, &here),
        }
    }

    fn check_named(
        &self,
        name: &str,
        value: &Value,
        path: &mut String,
        problems: &mut Vec<String>,
        here: &dyn Fn(&str) -> String,
    ) {
        match self.types.get(name) {
            Some(TypeDef::Input { fields, one_of }) => {
                let Some(object) = value.as_object() else {
                    problems.push(format!("{} must be an object (`{name}`)", here(path)));
                    return;
                };
                for key in object.keys() {
                    if !fields.iter().any(|field| &field.name == key) {
                        let known: Vec<&str> =
                            fields.iter().map(|field| field.name.as_str()).collect();
                        problems.push(format!(
                            "{} has no field `{key}` (`{name}` has {})",
                            here(path),
                            known.join(", ")
                        ));
                    }
                }
                if *one_of {
                    let given = fields
                        .iter()
                        .filter(|field| object.get(&field.name).is_some_and(|v| !v.is_null()))
                        .count();
                    if given != 1 {
                        problems.push(format!(
                            "{} must have exactly one field of `{name}`, it has {given}",
                            here(path)
                        ));
                    }
                }
                for field in fields {
                    let length = path.len();
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(&field.name);
                    match object.get(&field.name) {
                        Some(value) => self.check_value(&field.ty, value, path, problems),
                        None if field.required() && !*one_of => {
                            problems.push(format!("`{path}` is required"));
                        }
                        None => {}
                    }
                    path.truncate(length);
                }
            }
            Some(TypeDef::Enum(values)) => match value.as_str() {
                Some(text) if values.iter().any(|v| v == text) => {}
                _ => problems.push(format!(
                    "{} must be one of {} (`{name}`), got {value}",
                    here(path),
                    values.join(", ")
                )),
            },
            Some(TypeDef::Scalar) => {
                if let Some(expected) = scalar_mismatch(name, value) {
                    problems.push(format!("{} must be {expected}, got {value}", here(path)));
                }
            }
            _ => problems.push(format!("{} has the unknown type `{name}`", here(path))),
        }
    }
}

fn scalar_mismatch(name: &str, value: &Value) -> Option<&'static str> {
    let ok = match name {
        "Boolean" => value.is_boolean(),
        "Int" => value.is_i64() || value.is_u64(),
        "Float" => value.is_number(),
        "Decimal" => value.is_number() || value.as_str().is_some_and(|s| s.parse::<f64>().is_ok()),
        "JSON" => true,
        "Void" => value.is_null(),
        "ID"
        | "String"
        | "Handle"
        | "URL"
        | "Date"
        | "DateTime"
        | "DateTimeWithoutTimezone"
        | "TimeWithoutTimezone" => value.is_string() || (name == "ID" && value.is_number()),
        _ => true,
    };
    if ok {
        return None;
    }
    Some(match name {
        "Boolean" => "a boolean",
        "Int" => "an integer",
        "Float" => "a number",
        "Decimal" => "a decimal (a number or a numeric string)",
        _ => "a string",
    })
}

struct Walk<'s, 'f, 'q> {
    schema: &'s Schema,
    fragments: &'f HashMap<&'q str, &'q q::FragmentDefinition<'q, String>>,
    problems: Vec<String>,
    visiting: Vec<String>,
}

impl Walk<'_, '_, '_> {
    fn selection_set(
        &mut self,
        set: &SelectionSet<'_, String>,
        parent: &str,
        path: &mut Vec<String>,
    ) {
        for selection in &set.items {
            match selection {
                Selection::Field(field) => self.field(field, parent, path),
                Selection::InlineFragment(fragment) => {
                    let condition = fragment.type_condition.as_ref().map(|c| match c {
                        q::TypeCondition::On(name) => name.as_str(),
                    });
                    if let Some(inner) = self.condition(condition, parent, path) {
                        self.selection_set(&fragment.selection_set, inner, path);
                    }
                }
                Selection::FragmentSpread(spread) => {
                    let Some(fragment) = self.fragments.get(spread.fragment_name.as_str()) else {
                        self.problems.push(format!(
                            "the fragment `{}` is not defined",
                            spread.fragment_name
                        ));
                        continue;
                    };
                    if self.visiting.contains(&spread.fragment_name) {
                        continue;
                    }
                    let q::TypeCondition::On(name) = &fragment.type_condition;
                    if let Some(inner) = self.condition(Some(name), parent, path) {
                        self.visiting.push(spread.fragment_name.clone());
                        self.selection_set(&fragment.selection_set, inner, path);
                        self.visiting.pop();
                    }
                }
            }
        }
    }

    /// The type to look fields up in under a type condition, after checking it can apply.
    /// `None` when the type does not exist: what is selected under it is not checked.
    fn condition<'t>(
        &mut self,
        condition: Option<&'t str>,
        parent: &'t str,
        path: &[String],
    ) -> Option<&'t str> {
        let Some(condition) = condition else {
            return Some(parent);
        };
        if !self.schema.has_type(condition) {
            self.problems
                .push(format!("{}the type `{condition}` does not exist", at(path)));
            return None;
        }
        let a = self.schema.objects_of(parent);
        let b = self.schema.objects_of(condition);
        if parent != condition && a.is_disjoint(&b) {
            self.problems.push(format!(
                "{}`... on {condition}` can never apply to `{parent}`",
                at(path)
            ));
        }
        Some(condition)
    }

    fn field(&mut self, field: &q::Field<'_, String>, parent: &str, path: &mut Vec<String>) {
        if field.name == "__typename" {
            return;
        }
        path.push(field.name.clone());
        match self.schema.field(parent, &field.name) {
            None => {
                let known = self.schema.field_names(parent);
                let hint = if known.is_empty() {
                    String::new()
                } else {
                    format!(" (it has {})", known.join(", "))
                };
                self.problems.push(format!(
                    "`{}` does not exist on `{parent}`{hint}",
                    path.join(".")
                ));
            }
            Some(definition) => {
                for (name, _) in &field.arguments {
                    if !definition.args.iter().any(|arg| &arg.name == name) {
                        self.problems
                            .push(format!("`{}` has no argument `{name}`", path.join(".")));
                    }
                }
                for arg in definition.args.iter().filter(|arg| arg.required()) {
                    if !field.arguments.iter().any(|(name, _)| name == &arg.name) {
                        self.problems.push(format!(
                            "`{}` needs the argument `{}`",
                            path.join("."),
                            arg.name
                        ));
                    }
                }
                let ty = definition.ty.base();
                if field.selection_set.items.is_empty() {
                    if self.schema.has_composite(ty) {
                        self.problems.push(format!(
                            "`{}` is a `{ty}`: select some of its fields",
                            path.join(".")
                        ));
                    }
                } else if self.schema.has_composite(ty) {
                    self.selection_set(&field.selection_set, ty, path);
                } else {
                    self.problems.push(format!(
                        "`{}` is a `{ty}` and has no fields to select",
                        path.join(".")
                    ));
                }
            }
        }
        path.pop();
    }
}

fn at(path: &[String]) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("`{}`: ", path.join("."))
    }
}

impl Schema {
    fn has_composite(&self, name: &str) -> bool {
        matches!(
            self.types.get(name),
            Some(TypeDef::Object { .. } | TypeDef::Interface { .. } | TypeDef::Union(_))
        )
    }
}

fn type_def(definition: &s::TypeDefinition<'_, String>) -> (String, TypeDef) {
    match definition {
        s::TypeDefinition::Scalar(scalar) => (scalar.name.clone(), TypeDef::Scalar),
        s::TypeDefinition::Enum(en) => (
            en.name.clone(),
            TypeDef::Enum(en.values.iter().map(|value| value.name.clone()).collect()),
        ),
        s::TypeDefinition::Union(union) => {
            (union.name.clone(), TypeDef::Union(union.types.clone()))
        }
        s::TypeDefinition::Object(object) => (
            object.name.clone(),
            TypeDef::Object {
                fields: fields(&object.fields),
                interfaces: object.implements_interfaces.clone(),
            },
        ),
        s::TypeDefinition::Interface(interface) => (
            interface.name.clone(),
            TypeDef::Interface {
                fields: fields(&interface.fields),
            },
        ),
        s::TypeDefinition::InputObject(input) => (
            input.name.clone(),
            TypeDef::Input {
                fields: input
                    .fields
                    .iter()
                    .filter(|field| field.name != PLACEHOLDER_FIELD)
                    .map(input_field)
                    .collect(),
                one_of: input
                    .directives
                    .iter()
                    .any(|directive| directive.name == "oneOf"),
            },
        ),
    }
}

fn fields(fields: &[s::Field<'_, String>]) -> HashMap<String, Field> {
    fields
        .iter()
        .map(|field| {
            (
                field.name.clone(),
                Field {
                    ty: TypeRef::from(&field.field_type),
                    args: field.arguments.iter().map(input_field).collect(),
                },
            )
        })
        .collect()
}

/// A field the schema generator adds to an input object that has none (SDL cannot print an
/// empty one): see `tools/checkout/function_schemas.mjs`.
const PLACEHOLDER_FIELD: &str = "_noField";

fn input_field(value: &s::InputValue<'_, String>) -> InputField {
    InputField {
        name: value.name.clone(),
        ty: TypeRef::from(&value.value_type),
        default: value.default_value.as_ref().map(const_value),
    }
}

/// A constant GraphQL value (the default of an argument) as JSON.
fn const_value(value: &q::Value<'_, String>) -> Value {
    match value {
        q::Value::Variable(_) | q::Value::Null => Value::Null,
        q::Value::Int(number) => number.as_i64().map_or(Value::Null, Value::from),
        q::Value::Float(number) => {
            serde_json::Number::from_f64(*number).map_or(Value::Null, Value::Number)
        }
        q::Value::String(text) | q::Value::Enum(text) => Value::String(text.clone()),
        q::Value::Boolean(flag) => Value::Bool(*flag),
        q::Value::List(items) => Value::Array(items.iter().map(const_value).collect()),
        q::Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, item)| (key.clone(), const_value(item)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests;
