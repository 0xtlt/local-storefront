//! The schema as graphql-codegen sees it: the types of the SDL, sorted by name, with the fields,
//! arguments, input fields, enum values, interfaces and union members of each sorted too
//! (graphql-codegen sorts the schema it loads).

use std::collections::{HashMap, HashSet};

use graphql_parser::schema as s;

use super::text::natural_compare;

#[derive(Debug)]
pub(super) struct Model {
    pub types: Vec<TypeDef>,
    index: HashMap<String, usize>,
    pub query: Option<String>,
    pub mutation: Option<String>,
    pub subscription: Option<String>,
    /// The built-in scalars the schema refers to. graphql-js only has the built-in scalars the
    /// schema uses (`String` and `Boolean` always: the directives use them), and graphql-codegen
    /// only maps those.
    pub used_built_ins: HashSet<String>,
}

#[derive(Debug)]
pub(super) struct TypeDef {
    pub name: String,
    pub description: Option<String>,
    /// Whether the SDL declares the type (the five built-in scalars are not declared).
    pub declared: bool,
    pub kind: Kind,
}

#[derive(Debug)]
pub(super) enum Kind {
    Scalar,
    Object {
        interfaces: Vec<String>,
        fields: Vec<Field>,
    },
    Interface {
        fields: Vec<Field>,
    },
    Union {
        members: Vec<String>,
    },
    Enum {
        values: Vec<EnumValue>,
    },
    Input {
        fields: Vec<InputValue>,
        one_of: bool,
    },
}

#[derive(Debug)]
pub(super) struct Field {
    pub name: String,
    pub description: Option<String>,
    pub deprecation: Option<String>,
    pub args: Vec<InputValue>,
    pub ty: TypeRef,
}

#[derive(Debug)]
pub(super) struct InputValue {
    pub name: String,
    pub description: Option<String>,
    pub deprecation: Option<String>,
    pub ty: TypeRef,
    pub has_default: bool,
}

#[derive(Debug)]
pub(super) struct EnumValue {
    pub name: String,
    pub description: Option<String>,
    pub deprecation: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum TypeRef {
    Named(String),
    List(Box<TypeRef>),
    NonNull(Box<TypeRef>),
}

impl TypeRef {
    pub fn from_ast(ty: &s::Type<'_, String>) -> TypeRef {
        match ty {
            s::Type::NamedType(name) => TypeRef::Named(name.clone()),
            s::Type::ListType(inner) => TypeRef::List(Box::new(TypeRef::from_ast(inner))),
            s::Type::NonNullType(inner) => TypeRef::NonNull(Box::new(TypeRef::from_ast(inner))),
        }
    }

    /// The name of the type under the lists and the `!`s.
    pub fn base(&self) -> &str {
        match self {
            TypeRef::Named(name) => name,
            TypeRef::List(inner) | TypeRef::NonNull(inner) => inner.base(),
        }
    }

    pub fn is_non_null(&self) -> bool {
        matches!(self, TypeRef::NonNull(_))
    }
}

const BUILT_IN_SCALARS: [&str; 5] = ["ID", "String", "Boolean", "Int", "Float"];

impl Model {
    pub fn parse(sdl: &str) -> Result<Model, String> {
        let document = s::parse_schema::<String>(sdl)
            .map_err(|error| format!("the schema is not valid GraphQL: {error}"))?;
        let mut types = Vec::new();
        let mut extensions = Vec::new();
        let mut used: HashSet<String> = HashSet::new();
        let mut roots = (None, None, None);
        for definition in &document.definitions {
            match definition {
                s::Definition::SchemaDefinition(schema) => {
                    roots = (
                        schema.query.clone(),
                        schema.mutation.clone(),
                        schema.subscription.clone(),
                    );
                }
                s::Definition::TypeDefinition(definition) => types.push(type_def(definition)?),
                s::Definition::TypeExtension(extension) => extensions.push(extension),
                s::Definition::DirectiveDefinition(directive) => {
                    for arg in &directive.arguments {
                        used.insert(TypeRef::from_ast(&arg.value_type).base().to_string());
                    }
                }
            }
        }
        for extension in extensions {
            extend_type(&mut types, extension)?;
        }
        for name in BUILT_IN_SCALARS {
            if !types.iter().any(|ty| ty.name == name) {
                types.push(TypeDef {
                    name: name.to_string(),
                    description: None,
                    declared: false,
                    kind: Kind::Scalar,
                });
            }
        }
        types.sort_by(|a, b| natural_compare(&a.name, &b.name));
        let mut index = HashMap::new();
        for (position, ty) in types.iter().enumerate() {
            if index.insert(ty.name.clone(), position).is_some() {
                return Err(format!("the schema defines the type `{}` twice", ty.name));
            }
        }
        let mut model = Model {
            types,
            index,
            query: None,
            mutation: None,
            subscription: None,
            used_built_ins: HashSet::new(),
        };
        let has_schema_definition = document
            .definitions
            .iter()
            .any(|definition| matches!(definition, s::Definition::SchemaDefinition(_)));
        let root = |explicit: Option<String>, default: &str| {
            if has_schema_definition {
                explicit
            } else {
                model.get(default).map(|ty| ty.name.clone())
            }
        };
        let (query, mutation, subscription) = (
            root(roots.0.clone(), "Query"),
            root(roots.1.clone(), "Mutation"),
            root(roots.2.clone(), "Subscription"),
        );
        model.query = query;
        model.mutation = mutation;
        model.subscription = subscription;
        model.check_references()?;
        model.used_built_ins = BUILT_IN_SCALARS
            .into_iter()
            .filter(|name| used.contains(*name) || model.uses(name))
            .map(str::to_string)
            .collect();
        Ok(model)
    }

    pub fn get(&self, name: &str) -> Option<&TypeDef> {
        self.index.get(name).map(|&position| &self.types[position])
    }

    /// The type a field of `type_name` has, by the name of the field.
    pub fn field(&self, type_name: &str, field: &str) -> Option<&Field> {
        self.fields(type_name)?.iter().find(|f| f.name == field)
    }

    pub fn fields(&self, type_name: &str) -> Option<&[Field]> {
        match &self.get(type_name)?.kind {
            Kind::Object { fields, .. } | Kind::Interface { fields } => Some(fields),
            _ => None,
        }
    }

    /// The interfaces an object type implements (none for any other type), sorted.
    pub fn interfaces(&self, type_name: &str) -> &[String] {
        match self.get(type_name).map(|ty| &ty.kind) {
            Some(Kind::Object { interfaces, .. }) => interfaces,
            _ => &[],
        }
    }

    pub fn is_composite(&self, name: &str) -> bool {
        matches!(
            self.get(name).map(|ty| &ty.kind),
            Some(Kind::Object { .. } | Kind::Interface { .. } | Kind::Union { .. })
        )
    }

    pub fn is_abstract(&self, name: &str) -> bool {
        matches!(
            self.get(name).map(|ty| &ty.kind),
            Some(Kind::Interface { .. } | Kind::Union { .. })
        )
    }

    /// `getPossibleTypes` of graphql-codegen: the type itself for an object type, the members of a
    /// union, the objects that implement an interface (in the order of the sorted schema).
    pub fn possible_types(&self, name: &str) -> Vec<&str> {
        match self.get(name).map(|ty| &ty.kind) {
            Some(Kind::Object { .. }) => vec![self.get(name).unwrap().name.as_str()],
            Some(Kind::Union { members }) => members.iter().map(String::as_str).collect(),
            Some(Kind::Interface { .. }) => self
                .types
                .iter()
                .filter(|ty| match &ty.kind {
                    Kind::Object { interfaces, .. } => interfaces.iter().any(|i| i == name),
                    _ => false,
                })
                .map(|ty| ty.name.as_str())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Whether the object type `object` is the abstract type `other`, or one of its possible
    /// types.
    pub fn is_sub_type(&self, object: &str, other: &str) -> bool {
        object == other || (self.is_abstract(other) && self.possible_types(other).contains(&object))
    }

    pub fn root(&self, operation: &str) -> Option<&str> {
        match operation {
            "query" => self.query.as_deref(),
            "mutation" => self.mutation.as_deref(),
            _ => self.subscription.as_deref(),
        }
    }

    /// Whether a field, an argument or an input field of the schema has the type `name`.
    fn uses(&self, name: &str) -> bool {
        self.types.iter().any(|ty| match &ty.kind {
            Kind::Object { fields, .. } | Kind::Interface { fields } => {
                fields.iter().any(|field| {
                    field.ty.base() == name || field.args.iter().any(|arg| arg.ty.base() == name)
                })
            }
            Kind::Input { fields, .. } => fields.iter().any(|field| field.ty.base() == name),
            _ => false,
        })
    }

    fn check_references(&self) -> Result<(), String> {
        let check = |name: &str, user: &str| -> Result<(), String> {
            if self.get(name).is_some() {
                Ok(())
            } else {
                Err(format!(
                    "the schema uses the type `{name}` in `{user}`, and does not define it"
                ))
            }
        };
        for ty in &self.types {
            match &ty.kind {
                Kind::Scalar | Kind::Enum { .. } => {}
                Kind::Object { interfaces, fields } => {
                    for name in interfaces {
                        check(name, &ty.name)?;
                    }
                    for field in fields {
                        check(field.ty.base(), &format!("{}.{}", ty.name, field.name))?;
                        for arg in &field.args {
                            check(arg.ty.base(), &format!("{}.{}", ty.name, field.name))?;
                        }
                    }
                }
                Kind::Interface { fields } => {
                    for field in fields {
                        check(field.ty.base(), &format!("{}.{}", ty.name, field.name))?;
                        for arg in &field.args {
                            check(arg.ty.base(), &format!("{}.{}", ty.name, field.name))?;
                        }
                    }
                }
                Kind::Union { members } => {
                    for name in members {
                        check(name, &ty.name)?;
                    }
                }
                Kind::Input { fields, .. } => {
                    for field in fields {
                        check(field.ty.base(), &format!("{}.{}", ty.name, field.name))?;
                    }
                }
            }
        }
        for root in [&self.query, &self.mutation, &self.subscription]
            .into_iter()
            .flatten()
        {
            check(root, "the schema definition")?;
        }
        Ok(())
    }
}

/// Adds what an `extend` definition has to the type it extends.
fn extend_type(
    types: &mut [TypeDef],
    extension: &s::TypeExtension<'_, String>,
) -> Result<(), String> {
    let (name, found) = match extension {
        s::TypeExtension::Scalar(e) => (&e.name, "scalar"),
        s::TypeExtension::Object(e) => (&e.name, "type"),
        s::TypeExtension::Interface(e) => (&e.name, "interface"),
        s::TypeExtension::Union(e) => (&e.name, "union"),
        s::TypeExtension::Enum(e) => (&e.name, "enum"),
        s::TypeExtension::InputObject(e) => (&e.name, "input"),
    };
    let ty = types
        .iter_mut()
        .find(|ty| ty.name == *name)
        .ok_or_else(|| format!("the schema extends the type `{name}`, and does not define it"))?;
    let twice = |what: &str, item: &str| format!("the schema defines {what} `{name}.{item}` twice");
    match (extension, &mut ty.kind) {
        (s::TypeExtension::Scalar(_), Kind::Scalar) => {}
        (
            s::TypeExtension::Object(e),
            Kind::Object {
                interfaces,
                fields: existing,
            },
        ) => {
            interfaces.extend(e.implements_interfaces.iter().cloned());
            interfaces.sort_by(|a, b| natural_compare(a, b));
            for field in self::fields(&e.fields)? {
                if existing.iter().any(|other| other.name == field.name) {
                    return Err(twice("the field", &field.name));
                }
                existing.push(field);
            }
            existing.sort_by(|a, b| natural_compare(&a.name, &b.name));
        }
        (s::TypeExtension::Interface(e), Kind::Interface { fields: existing }) => {
            for field in self::fields(&e.fields)? {
                if existing.iter().any(|other| other.name == field.name) {
                    return Err(twice("the field", &field.name));
                }
                existing.push(field);
            }
            existing.sort_by(|a, b| natural_compare(&a.name, &b.name));
        }
        (s::TypeExtension::Union(e), Kind::Union { members }) => {
            members.extend(e.types.iter().cloned());
            members.sort_by(|a, b| natural_compare(a, b));
        }
        (s::TypeExtension::Enum(e), Kind::Enum { values }) => {
            for value in &e.values {
                if values.iter().any(|other| other.name == value.name) {
                    return Err(twice("the value", &value.name));
                }
                values.push(EnumValue {
                    name: value.name.clone(),
                    description: value.description.clone(),
                    deprecation: deprecation(&value.directives)?,
                });
            }
            values.sort_by(|a, b| natural_compare(&a.name, &b.name));
        }
        (
            s::TypeExtension::InputObject(e),
            Kind::Input {
                fields: existing,
                one_of,
            },
        ) => {
            for field in input_values(&e.fields)? {
                if existing.iter().any(|other| other.name == field.name) {
                    return Err(twice("the field", &field.name));
                }
                existing.push(field);
            }
            existing.sort_by(|a, b| natural_compare(&a.name, &b.name));
            *one_of |= e
                .directives
                .iter()
                .any(|directive| directive.name == "oneOf");
        }
        _ => {
            return Err(format!(
                "the schema extends `{name}` as a {found}, which it is not"
            ));
        }
    }
    Ok(())
}

fn sorted<T>(mut items: Vec<T>, name: impl Fn(&T) -> &str) -> Vec<T> {
    items.sort_by(|a, b| natural_compare(name(a), name(b)));
    items
}

fn type_def(definition: &s::TypeDefinition<'_, String>) -> Result<TypeDef, String> {
    Ok(match definition {
        s::TypeDefinition::Scalar(scalar) => TypeDef {
            name: scalar.name.clone(),
            description: scalar.description.clone(),
            declared: true,
            kind: Kind::Scalar,
        },
        s::TypeDefinition::Object(object) => TypeDef {
            name: object.name.clone(),
            description: object.description.clone(),
            declared: true,
            kind: Kind::Object {
                interfaces: sorted(object.implements_interfaces.clone(), |name| name),
                fields: fields(&object.fields)?,
            },
        },
        s::TypeDefinition::Interface(interface) => TypeDef {
            name: interface.name.clone(),
            description: interface.description.clone(),
            declared: true,
            kind: Kind::Interface {
                fields: fields(&interface.fields)?,
            },
        },
        s::TypeDefinition::Union(union) => TypeDef {
            name: union.name.clone(),
            description: union.description.clone(),
            declared: true,
            kind: Kind::Union {
                members: sorted(union.types.clone(), |name| name),
            },
        },
        s::TypeDefinition::Enum(enumeration) => TypeDef {
            name: enumeration.name.clone(),
            description: enumeration.description.clone(),
            declared: true,
            kind: Kind::Enum {
                values: sorted(
                    enumeration
                        .values
                        .iter()
                        .map(|value| {
                            Ok(EnumValue {
                                name: value.name.clone(),
                                description: value.description.clone(),
                                deprecation: deprecation(&value.directives)?,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?,
                    |value| &value.name,
                ),
            },
        },
        s::TypeDefinition::InputObject(input) => TypeDef {
            name: input.name.clone(),
            description: input.description.clone(),
            declared: true,
            kind: Kind::Input {
                fields: input_values(&input.fields)?,
                one_of: input
                    .directives
                    .iter()
                    .any(|directive| directive.name == "oneOf"),
            },
        },
    })
}

fn fields(fields: &[s::Field<'_, String>]) -> Result<Vec<Field>, String> {
    let fields = fields
        .iter()
        .map(|field| {
            Ok(Field {
                name: field.name.clone(),
                description: field.description.clone(),
                deprecation: deprecation(&field.directives)?,
                args: input_values(&field.arguments)?,
                ty: TypeRef::from_ast(&field.field_type),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(sorted(fields, |field| &field.name))
}

fn input_values(values: &[s::InputValue<'_, String>]) -> Result<Vec<InputValue>, String> {
    let values = values
        .iter()
        .map(|value| {
            Ok(InputValue {
                name: value.name.clone(),
                description: value.description.clone(),
                deprecation: deprecation(&value.directives)?,
                ty: TypeRef::from_ast(&value.value_type),
                has_default: value.default_value.is_some(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(sorted(values, |value| &value.name))
}

/// The reason of a `@deprecated` directive, as graphql-codegen writes it in a comment: the value
/// of its first argument, or `Field no longer supported` without one.
fn deprecation(directives: &[s::Directive<'_, String>]) -> Result<Option<String>, String> {
    let Some(directive) = directives
        .iter()
        .find(|directive| directive.name == "deprecated")
    else {
        return Ok(None);
    };
    match directive.arguments.first() {
        None => Ok(Some("Field no longer supported".to_string())),
        Some((_, s::Value::String(reason))) => Ok(Some(reason.clone())),
        Some((name, _)) => Err(format!(
            "the schema has a `@deprecated` with a `{name}` that is not a string"
        )),
    }
}
