//! The shape checker: every record of a shop is in the shape of its GraphQL type. Each key is a
//! field of the type, each value fits the type of the field: a scalar of the right form, an enum
//! value, a nested object of the right type, or the GID of a record of a compatible type.
//!
//! It reads the schema the engine serves, `schemas/2026-10/admin.graphql`; without it, the
//! check is skipped (with a message saying how to get the file).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use graphql_parser::schema::{Definition, Document, Type, TypeDefinition};
use lsf_admin::state::{Record, ShopState};
use serde_json::Value;

/// What the checker knows of a type.
enum Kind {
    Scalar,
    Enum(HashSet<String>),
    Object {
        fields: HashMap<String, Type<'static, String>>,
        interfaces: Vec<String>,
    },
    Interface {
        fields: HashMap<String, Type<'static, String>>,
    },
    Union(HashSet<String>),
    Input,
}

pub struct Schema {
    types: HashMap<String, Kind>,
}

/// Where the schema is.
pub fn schema_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas/2026-10/admin.graphql")
}

impl Schema {
    /// The schema of the Admin API, or `None` (and a message) when the file is not there.
    pub fn load() -> Option<Schema> {
        let path = schema_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!(
                "skipped: the shape check needs {} (generated from the pinned \
                 @shopify/dev-mcp by tools/admin/admin_schema.mjs), which is absent",
                path.display()
            );
            return None;
        };
        let document: Document<'static, String> = graphql_parser::parse_schema::<String>(&text)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
            .into_static();
        let mut types = HashMap::new();
        for definition in document.definitions {
            let Definition::TypeDefinition(definition) = definition else {
                continue;
            };
            let fields = |fields: Vec<graphql_parser::schema::Field<'static, String>>| {
                fields
                    .into_iter()
                    .map(|field| (field.name, field.field_type))
                    .collect()
            };
            let (name, kind) = match definition {
                TypeDefinition::Scalar(scalar) => (scalar.name, Kind::Scalar),
                TypeDefinition::Enum(definition) => (
                    definition.name,
                    Kind::Enum(definition.values.into_iter().map(|v| v.name).collect()),
                ),
                TypeDefinition::Object(object) => (
                    object.name,
                    Kind::Object {
                        fields: fields(object.fields),
                        interfaces: object.implements_interfaces,
                    },
                ),
                TypeDefinition::Interface(interface) => (
                    interface.name,
                    Kind::Interface {
                        fields: fields(interface.fields),
                    },
                ),
                TypeDefinition::Union(union) => {
                    (union.name, Kind::Union(union.types.into_iter().collect()))
                }
                TypeDefinition::InputObject(input) => (input.name, Kind::Input),
            };
            types.insert(name, kind);
        }
        for builtin in ["String", "Int", "Float", "Boolean", "ID"] {
            types.insert(builtin.to_string(), Kind::Scalar);
        }
        Some(Schema { types })
    }

    /// Every problem of the records of a shop, as `<record key>.<path>: <problem>`.
    pub fn check(&self, state: &ShopState) -> Vec<String> {
        let mut problems = Vec::new();
        for (key, record) in state.iter() {
            let type_name = record
                .get("__typename")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut checker = Checker {
                schema: self,
                state,
                problems: &mut problems,
            };
            match self.types.get(type_name) {
                Some(Kind::Object { fields, .. }) => {
                    // A record is keyed by its `id`, when its type has one.
                    if fields.contains_key("id")
                        && record.get("id").and_then(Value::as_str) != Some(key)
                    {
                        checker.problem(key, "its key is not its `id`".to_string());
                    }
                    checker.object(key, type_name, record);
                }
                _ => checker.problem(key, format!("`{type_name}` is not an object type")),
            }
        }
        problems
    }

    /// Whether a record of type `actual` can stand where `expected` is asked.
    fn compatible(&self, actual: &str, expected: &str) -> bool {
        if actual == expected {
            return true;
        }
        match self.types.get(expected) {
            Some(Kind::Union(members)) => members.contains(actual),
            Some(Kind::Interface { .. }) => matches!(
                self.types.get(actual),
                Some(Kind::Object { interfaces, .. }) if interfaces.iter().any(|i| i == expected)
            ),
            _ => false,
        }
    }

    /// The node type of a connection type, if it is one.
    fn node_of(&self, connection: &str) -> Option<String> {
        if !connection.ends_with("Connection") {
            return None;
        }
        let Some(Kind::Object { fields, .. }) = self.types.get(connection) else {
            return None;
        };
        let nodes = fields.get("nodes")?;
        Some(named(nodes).to_string())
    }
}

struct Checker<'s> {
    schema: &'s Schema,
    state: &'s ShopState,
    problems: &'s mut Vec<String>,
}

impl Checker<'_> {
    fn problem(&mut self, path: &str, problem: String) {
        self.problems.push(format!("{path}: {problem}"));
    }

    /// The fields of an object (a record or a nested one) of type `type_name`.
    fn object(&mut self, path: &str, type_name: &str, object: &Record) {
        let Some(Kind::Object { fields, .. }) = self.schema.types.get(type_name) else {
            self.problem(path, format!("`{type_name}` is not an object type"));
            return;
        };
        if let Some(name) = object.get("__typename").and_then(Value::as_str)
            && name != type_name
        {
            self.problem(
                path,
                format!("`__typename` is `{name}`, the field is a `{type_name}`"),
            );
        }
        for (field, value) in object {
            // `lsf…` keys hold what lsf keeps beyond the record, which GraphQL never reads.
            if field == "__typename" || field.starts_with("lsf") {
                continue;
            }
            let path = format!("{path}.{field}");
            match fields.get(field) {
                Some(field_type) => self.value(&path, field_type, value),
                None => self.problem(&path, format!("`{type_name}` has no field `{field}`")),
            }
        }
    }

    fn value(&mut self, path: &str, field_type: &Type<'static, String>, value: &Value) {
        match field_type {
            Type::NonNullType(inner) => {
                if value.is_null() {
                    self.problem(path, "null for a non-null field".to_string());
                } else {
                    self.value(path, inner, value);
                }
            }
            _ if value.is_null() => {}
            Type::ListType(inner) => match value {
                Value::Array(items) => {
                    for (index, item) in items.iter().enumerate() {
                        self.value(&format!("{path}[{index}]"), inner, item);
                    }
                }
                _ => self.problem(path, format!("a list is expected, got {value}")),
            },
            Type::NamedType(name) => self.named(path, name, value),
        }
    }

    fn named(&mut self, path: &str, name: &str, value: &Value) {
        if let Some(node) = self.schema.node_of(name) {
            // A connection: the GIDs of its records, or its nodes inline.
            match value {
                Value::Array(items) => {
                    for (index, item) in items.iter().enumerate() {
                        self.named(&format!("{path}[{index}]"), &node, item);
                    }
                }
                _ => self.problem(path, format!("a connection is an array, got {value}")),
            }
            return;
        }
        match self.schema.types.get(name) {
            None => self.problem(path, format!("unknown type `{name}`")),
            Some(Kind::Scalar) => {
                if let Some(problem) = scalar(name, value) {
                    self.problem(path, problem);
                }
            }
            Some(Kind::Enum(values)) => match value.as_str() {
                Some(text) if values.contains(text) => {}
                _ => self.problem(path, format!("{value} is not a value of `{name}`")),
            },
            Some(Kind::Input) => self.problem(path, format!("`{name}` is an input type")),
            Some(Kind::Object { .. } | Kind::Interface { .. } | Kind::Union(_)) => match value {
                Value::String(key) => match self.state.type_of(key) {
                    Some(actual) if self.schema.compatible(actual, name) => {}
                    Some(actual) => self.problem(
                        path,
                        format!("{key} is a `{actual}`, the field is a `{name}`"),
                    ),
                    None => self.problem(path, format!("{key} is no record of the shop")),
                },
                Value::Object(object) => {
                    let concrete = match self.schema.types.get(name) {
                        Some(Kind::Object { .. }) => Some(name.to_string()),
                        _ => object
                            .get("__typename")
                            .and_then(Value::as_str)
                            .filter(|actual| self.schema.compatible(actual, name))
                            .map(str::to_string),
                    };
                    match concrete {
                        Some(concrete) => self.object(path, &concrete, object),
                        None => self.problem(
                            path,
                            format!("an object of the abstract `{name}` needs a `__typename` of one of its types"),
                        ),
                    }
                }
                _ => self.problem(
                    path,
                    format!("an object of `{name}` is expected, got {value}"),
                ),
            },
        }
    }
}

/// The named type of a type.
fn named<'t>(field_type: &'t Type<'static, String>) -> &'t str {
    match field_type {
        Type::NamedType(name) => name,
        Type::ListType(inner) | Type::NonNullType(inner) => named(inner),
    }
}

/// What is wrong with a scalar, if anything.
fn scalar(name: &str, value: &Value) -> Option<String> {
    let text = value.as_str();
    let ok = match name {
        "Int" => value.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
        "Float" => value.is_number(),
        "Boolean" => value.is_boolean(),
        "JSON" => true,
        "Money" => text.is_some_and(|t| decimal(t) == Some(2)),
        "Decimal" => text.is_some_and(|t| decimal(t).is_some_and(|places| places >= 1)),
        "UnsignedInt64" | "BigInt" => {
            text.is_some_and(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
        }
        "DateTime" => text
            .is_some_and(|t| t.ends_with('Z') && chrono::DateTime::parse_from_rfc3339(t).is_ok()),
        "Date" => text.is_some_and(|t| chrono::NaiveDate::parse_from_str(t, "%Y-%m-%d").is_ok()),
        "URL" => text.is_some_and(|t| t.starts_with("https://") || t.starts_with("http://")),
        "Color" => text.is_some_and(|t| t.starts_with('#')),
        "UtcOffset" => {
            text.is_some_and(|t| t.len() == 6 && (t.starts_with('+') || t.starts_with('-')))
        }
        _ => text.is_some(),
    };
    (!ok).then(|| format!("{value} is not a `{name}`"))
}

/// The number of decimal places of a decimal string, `None` when it is not one.
fn decimal(text: &str) -> Option<usize> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = digits.split_once('.')?;
    let all_digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    (all_digits(whole) && all_digits(fraction)).then_some(fraction.len())
}
