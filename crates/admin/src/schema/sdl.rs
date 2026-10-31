//! Builds a [`Schema`] from SDL, with graphql-parser: the SDL of the types, then that of the
//! introspection types, which an SDL document may not declare (their names start with `__`) but
//! which graphql-parser reads all the same.

use std::collections::HashMap;

use graphql_parser::schema as s;
use indexmap::IndexMap;
use serde_json::{Map, Number, Value};

use super::{
    DirectiveDef, DirectiveLocation, EnumValue, Field, InputValue, Kind, Schema, TypeDef, TypeRef,
};

/// The reason `@deprecated` gives when it is not given one (its default in the spec).
const NO_LONGER_SUPPORTED: &str = "No longer supported";

pub(super) fn build(sdl: &str, introspection: &str) -> Result<Schema, String> {
    let mut schema = Schema {
        description: None,
        query_type: "Query".to_string(),
        mutation_type: None,
        subscription_type: None,
        types: IndexMap::new(),
        directives: IndexMap::new(),
        connections: HashMap::new(),
        meta: meta_fields(),
    };
    for source in [sdl, introspection] {
        let document = s::parse_schema::<&str>(source).map_err(|error| error.to_string())?;
        for definition in document.definitions {
            match definition {
                s::Definition::SchemaDefinition(root) => {
                    if let Some(query) = root.query {
                        schema.query_type = query.to_string();
                    }
                    schema.mutation_type = root.mutation.map(str::to_string);
                    schema.subscription_type = root.subscription.map(str::to_string);
                }
                s::Definition::TypeDefinition(definition) => {
                    let def = type_def(definition);
                    schema.types.insert(def.name.clone(), def);
                }
                s::Definition::DirectiveDefinition(directive) => {
                    let def = DirectiveDef {
                        name: directive.name.to_string(),
                        description: directive.description,
                        locations: directive
                            .locations
                            .iter()
                            .map(|l| location(l.clone()))
                            .collect(),
                        args: input_values(directive.arguments),
                        repeatable: directive.repeatable,
                    };
                    schema.directives.insert(def.name.clone(), def);
                }
                s::Definition::TypeExtension(_) => {
                    return Err("the schema has a type extension, which lsf does not read".into());
                }
            }
        }
    }
    // The built-in scalars, when the SDL does not declare them.
    for name in ["String", "Int", "Float", "Boolean", "ID"] {
        if !schema.types.contains_key(name) {
            let def = empty(name.to_string(), Kind::Scalar, None);
            schema.types.insert(def.name.clone(), def);
        }
    }

    // The object types of each interface, by name, as introspection lists them.
    let mut implementations: HashMap<String, Vec<String>> = HashMap::new();
    for def in schema.types.values().filter(|def| def.kind == Kind::Object) {
        for interface in &def.interfaces {
            implementations
                .entry(interface.clone())
                .or_default()
                .push(def.name.clone());
        }
    }
    for (interface, mut objects) in implementations {
        objects.sort();
        if let Some(def) = schema.types.get_mut(&interface) {
            def.possible_types = objects;
        }
    }

    // The connections and the type of their nodes.
    let mut connections = HashMap::new();
    for def in schema.types.values().filter(|def| def.kind == Kind::Object) {
        let (Some(edges), Some(_)) = (def.field("edges"), def.field("pageInfo")) else {
            continue;
        };
        let node = schema
            .types
            .get(edges.ty.named())
            .and_then(|edge| edge.field("node"));
        if let Some(node) = node {
            connections.insert(def.name.clone(), node.ty.named().to_string());
        }
    }
    schema.connections = connections;

    for name in std::iter::once(&schema.query_type)
        .chain(&schema.mutation_type)
        .chain(&schema.subscription_type)
    {
        if schema.types.get(name).map(|def| def.kind) != Some(Kind::Object) {
            return Err(format!("the root type {name} is not an object type"));
        }
    }
    Ok(schema)
}

/// `__typename`, `__schema` and `__type`, as graphql-ruby declares them.
fn meta_fields() -> [Field; 3] {
    let non_null = |name: &str| TypeRef::NonNull(Box::new(TypeRef::Named(name.to_string())));
    let field = |name: &str, description: &str, ty: TypeRef| Field {
        name: name.to_string(),
        description: Some(description.to_string()),
        args: IndexMap::new(),
        ty,
        deprecation: None,
    };
    let mut ty = field(
        "__type",
        "A type in the GraphQL system",
        TypeRef::Named("__Type".to_string()),
    );
    let name = InputValue {
        name: "name".to_string(),
        description: None,
        ty: non_null("String"),
        default_value: None,
        default_literal: None,
        deprecation: None,
    };
    ty.args.insert(name.name.clone(), name);
    [
        field("__typename", "The name of this type", non_null("String")),
        field("__schema", "This GraphQL schema", non_null("__Schema")),
        ty,
    ]
}

fn empty(name: String, kind: Kind, description: Option<String>) -> TypeDef {
    TypeDef {
        name,
        kind,
        description,
        fields: IndexMap::new(),
        interfaces: Vec::new(),
        possible_types: Vec::new(),
        enum_values: IndexMap::new(),
        input_fields: IndexMap::new(),
        one_of: false,
        specified_by_url: None,
    }
}

fn type_def<'a>(definition: s::TypeDefinition<'a, &'a str>) -> TypeDef {
    match definition {
        s::TypeDefinition::Scalar(scalar) => {
            let mut def = empty(scalar.name.to_string(), Kind::Scalar, scalar.description);
            def.specified_by_url = scalar
                .directives
                .iter()
                .find(|directive| directive.name == "specifiedBy")
                .and_then(|directive| string_argument(directive, "url"));
            def
        }
        s::TypeDefinition::Object(object) => {
            let mut def = empty(object.name.to_string(), Kind::Object, object.description);
            def.interfaces = object
                .implements_interfaces
                .iter()
                .map(|n| n.to_string())
                .collect();
            def.fields = fields(object.fields);
            def
        }
        s::TypeDefinition::Interface(interface) => {
            let mut def = empty(
                interface.name.to_string(),
                Kind::Interface,
                interface.description,
            );
            def.interfaces = interface
                .implements_interfaces
                .iter()
                .map(|n| n.to_string())
                .collect();
            def.fields = fields(interface.fields);
            def
        }
        s::TypeDefinition::Union(union) => {
            let mut def = empty(union.name.to_string(), Kind::Union, union.description);
            def.possible_types = union.types.iter().map(|n| n.to_string()).collect();
            def
        }
        s::TypeDefinition::Enum(enumeration) => {
            let mut def = empty(
                enumeration.name.to_string(),
                Kind::Enum,
                enumeration.description,
            );
            def.enum_values = enumeration
                .values
                .into_iter()
                .map(|value| {
                    let value = EnumValue {
                        name: value.name.to_string(),
                        deprecation: deprecation(&value.directives),
                        description: value.description,
                    };
                    (value.name.clone(), value)
                })
                .collect();
            def
        }
        s::TypeDefinition::InputObject(input) => {
            let mut def = empty(input.name.to_string(), Kind::InputObject, input.description);
            def.one_of = input.directives.iter().any(|d| d.name == "oneOf");
            def.input_fields = input_values(input.fields);
            def
        }
    }
}

fn fields<'a>(fields: Vec<s::Field<'a, &'a str>>) -> IndexMap<String, Field> {
    fields
        .into_iter()
        .map(|field| {
            let field = Field {
                name: field.name.to_string(),
                ty: type_ref(&field.field_type),
                deprecation: deprecation(&field.directives),
                args: input_values(field.arguments),
                description: field.description,
            };
            (field.name.clone(), field)
        })
        .collect()
}

fn input_values<'a>(values: Vec<s::InputValue<'a, &'a str>>) -> IndexMap<String, InputValue> {
    values
        .into_iter()
        .map(|value| {
            let input = InputValue {
                name: value.name.to_string(),
                ty: type_ref(&value.value_type),
                default_value: value.default_value.as_ref().map(json),
                default_literal: value.default_value.as_ref().map(literal),
                deprecation: deprecation(&value.directives),
                description: value.description,
            };
            (input.name.clone(), input)
        })
        .collect()
}

fn type_ref<'a>(ty: &s::Type<'a, &'a str>) -> TypeRef {
    match ty {
        s::Type::NamedType(name) => TypeRef::Named(name.to_string()),
        s::Type::ListType(inner) => TypeRef::List(Box::new(type_ref(inner))),
        s::Type::NonNullType(inner) => TypeRef::NonNull(Box::new(type_ref(inner))),
    }
}

/// The reason of `@deprecated`, when there is one.
fn deprecation<'a>(directives: &[s::Directive<'a, &'a str>]) -> Option<String> {
    let directive = directives.iter().find(|d| d.name == "deprecated")?;
    Some(string_argument(directive, "reason").unwrap_or_else(|| NO_LONGER_SUPPORTED.to_string()))
}

fn string_argument<'a>(directive: &s::Directive<'a, &'a str>, name: &str) -> Option<String> {
    directive
        .arguments
        .iter()
        .find_map(|(arg, value)| match value {
            s::Value::String(text) if *arg == name => Some(text.clone()),
            _ => None,
        })
}

/// A constant value of the SDL as JSON: an enum value is its name.
fn json<'a>(value: &s::Value<'a, &'a str>) -> Value {
    match value {
        s::Value::Int(number) => number.as_i64().map_or(Value::Null, Value::from),
        s::Value::Float(number) => Number::from_f64(*number).map_or(Value::Null, Value::Number),
        s::Value::String(text) => Value::String(text.clone()),
        s::Value::Boolean(flag) => Value::Bool(*flag),
        s::Value::Null | s::Value::Variable(_) => Value::Null,
        s::Value::Enum(name) => Value::String(name.to_string()),
        s::Value::List(items) => Value::Array(items.iter().map(json).collect()),
        s::Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.to_string(), json(value)))
                .collect::<Map<_, _>>(),
        ),
    }
}

/// A constant value of the SDL as introspection prints it (`defaultValue`): graphql-ruby's
/// printer, which writes `"text"`, `[A, B]` and `{key: value}`.
fn literal<'a>(value: &s::Value<'a, &'a str>) -> String {
    match value {
        s::Value::Int(number) => number.as_i64().unwrap_or_default().to_string(),
        s::Value::Float(number) => crate::graphql::print::float(*number),
        s::Value::String(text) => serde_json::to_string(text).unwrap_or_default(),
        s::Value::Boolean(flag) => flag.to_string(),
        s::Value::Null => "null".to_string(),
        s::Value::Variable(name) => format!("${name}"),
        s::Value::Enum(name) => name.to_string(),
        s::Value::List(items) => {
            let items: Vec<String> = items.iter().map(literal).collect();
            format!("[{}]", items.join(", "))
        }
        s::Value::Object(fields) => {
            let fields: Vec<String> = fields
                .iter()
                .map(|(key, value)| format!("{key}: {}", literal(value)))
                .collect();
            format!("{{{}}}", fields.join(", "))
        }
    }
}

fn location(location: s::DirectiveLocation) -> DirectiveLocation {
    use s::DirectiveLocation as L;
    match location {
        L::Query => DirectiveLocation::Query,
        L::Mutation => DirectiveLocation::Mutation,
        L::Subscription => DirectiveLocation::Subscription,
        L::Field => DirectiveLocation::Field,
        L::FragmentDefinition => DirectiveLocation::FragmentDefinition,
        L::FragmentSpread => DirectiveLocation::FragmentSpread,
        L::InlineFragment => DirectiveLocation::InlineFragment,
        L::VariableDefinition => DirectiveLocation::VariableDefinition,
        L::Schema => DirectiveLocation::Schema,
        L::Scalar => DirectiveLocation::Scalar,
        L::Object => DirectiveLocation::Object,
        L::FieldDefinition => DirectiveLocation::FieldDefinition,
        L::ArgumentDefinition => DirectiveLocation::ArgumentDefinition,
        L::Interface => DirectiveLocation::Interface,
        L::Union => DirectiveLocation::Union,
        L::Enum => DirectiveLocation::Enum,
        L::EnumValue => DirectiveLocation::EnumValue,
        L::InputObject => DirectiveLocation::InputObject,
        L::InputFieldDefinition => DirectiveLocation::InputFieldDefinition,
    }
}
