//! The schema of the GraphQL Admin API, parsed once from the SDL that
//! `tools/admin/admin_schema.mjs` writes from Shopify's own introspection
//! (`schemas/admin/<version>/admin.graphql`): every type, field, argument, default value,
//! deprecation and description, in the order Shopify's introspection lists them.
//!
//! The executor validates queries against it and answers introspection from it; resolvers use it
//! to read the type of a field ([`Schema::field`]), to tell a connection from a list
//! ([`Schema::connection_node`]), or to list the values of an enum ([`Schema::enum_values`]).

mod sdl;

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use indexmap::IndexMap;
use serde_json::Value;

use crate::admin::version::{self, ApiVersion};

/// The schema of a version of the Admin API, parsed the first time a request of the version asks
/// for it: from `schemas/admin/<version>/admin.graphql.br`, the SDL Brotli-compressed by
/// `tools/admin/admin_schema.mjs` (its 4 MB take 0.4 MB of the binary, and are read back in
/// about 5 ms), and `introspection.graphql`. Validation, execution, introspection and cost of a
/// request use the schema of its version.
pub fn for_version(version: ApiVersion) -> &'static Schema {
    static PARSED: [OnceLock<Schema>; version::COUNT] = [const { OnceLock::new() }; version::COUNT];
    PARSED[version.index()].get_or_init(|| {
        let files = version.files();
        let sdl = decompress(files.schema);
        Schema::parse(&sdl, files.introspection)
            .unwrap_or_else(|error| panic!("the Admin schema {version} of lsf parses: {error}"))
    })
}

/// The schema of the default version ([`crate::admin::version::DEFAULT`]): what code that does not know
/// the version of its request uses. A resolver uses the schema of its request
/// (`ShopResolver::schema`, `FieldContext::schema`).
pub fn admin() -> &'static Schema {
    for_version(ApiVersion::default_version())
}

/// The SDL of a compressed schema file.
fn decompress(compressed: &[u8]) -> String {
    let mut sdl = Vec::with_capacity(compressed.len() * 11);
    brotli::BrotliDecompress(&mut std::io::Cursor::new(compressed), &mut sdl)
        .expect("the Admin schema of lsf decompresses");
    String::from_utf8(sdl).expect("the Admin schema of lsf is UTF-8")
}

/// A GraphQL schema.
#[derive(Debug)]
pub struct Schema {
    description: Option<String>,
    query_type: String,
    mutation_type: Option<String>,
    subscription_type: Option<String>,
    /// Every type, in the order of the SDL (the order Shopify's introspection lists them in), the
    /// introspection types last.
    types: IndexMap<String, TypeDef>,
    directives: IndexMap<String, DirectiveDef>,
    /// For each connection type, the type of its nodes.
    connections: HashMap<String, String>,
    /// The fields every query may select without the schema declaring them: `__typename` on
    /// every object, interface and union, `__schema` and `__type(name:)` on the query root.
    meta: [Field; 3],
}

/// What kind of type a type is, as introspection names it (`__TypeKind`, without the
/// wrapping kinds).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Scalar,
    Object,
    Interface,
    Union,
    Enum,
    InputObject,
}

impl Kind {
    /// The name of the kind in `__TypeKind`: `OBJECT`.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Scalar => "SCALAR",
            Kind::Object => "OBJECT",
            Kind::Interface => "INTERFACE",
            Kind::Union => "UNION",
            Kind::Enum => "ENUM",
            Kind::InputObject => "INPUT_OBJECT",
        }
    }
}

/// A named type. Which of its parts are filled depends on its kind: `fields` and `interfaces`
/// for objects and interfaces, `possible_types` for interfaces and unions, `enum_values` for
/// enums, `input_fields` for input objects.
#[derive(Debug)]
pub struct TypeDef {
    pub name: String,
    pub kind: Kind,
    pub description: Option<String>,
    pub fields: IndexMap<String, Field>,
    pub interfaces: Vec<String>,
    /// The object types a value of an interface or a union can be, by name. For an interface,
    /// the object types that declare it.
    pub possible_types: Vec<String>,
    pub enum_values: IndexMap<String, EnumValue>,
    pub input_fields: IndexMap<String, InputValue>,
    /// An input object marked `@oneOf`: exactly one of its fields is given, and not `null`.
    pub one_of: bool,
    /// `@specifiedBy(url:)` of a scalar.
    pub specified_by_url: Option<String>,
}

impl TypeDef {
    /// A field of an object or an interface.
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.get(name)
    }

    /// A field of an input object.
    pub fn input_field(&self, name: &str) -> Option<&InputValue> {
        self.input_fields.get(name)
    }

    /// A scalar or an enum: a field of this type has no selection.
    pub fn is_leaf(&self) -> bool {
        matches!(self.kind, Kind::Scalar | Kind::Enum)
    }

    /// An object, an interface or a union: a field of this type has a selection.
    pub fn is_composite(&self) -> bool {
        matches!(self.kind, Kind::Object | Kind::Interface | Kind::Union)
    }

    /// An interface or a union: the object type of a value is only known when it is resolved.
    pub fn is_abstract(&self) -> bool {
        matches!(self.kind, Kind::Interface | Kind::Union)
    }

    /// A scalar, an enum or an input object: a type a variable or an argument can have.
    pub fn is_input(&self) -> bool {
        matches!(self.kind, Kind::Scalar | Kind::Enum | Kind::InputObject)
    }
}

/// A field of an object or an interface.
#[derive(Debug)]
pub struct Field {
    pub name: String,
    pub description: Option<String>,
    pub args: IndexMap<String, InputValue>,
    pub ty: TypeRef,
    /// The reason of `@deprecated`, when the field is deprecated.
    pub deprecation: Option<String>,
}

impl Field {
    pub fn arg(&self, name: &str) -> Option<&InputValue> {
        self.args.get(name)
    }
}

/// An argument of a field or a directive, or a field of an input object.
#[derive(Debug)]
pub struct InputValue {
    pub name: String,
    pub description: Option<String>,
    pub ty: TypeRef,
    /// The default value, as JSON (an enum value is its name).
    pub default_value: Option<Value>,
    /// The default value as a GraphQL literal, as introspection writes it (`defaultValue`).
    pub default_literal: Option<String>,
    pub deprecation: Option<String>,
}

impl InputValue {
    /// Whether the value must be given: a non-null type without a default.
    pub fn is_required(&self) -> bool {
        self.ty.is_non_null() && self.default_value.is_none()
    }
}

/// A value of an enum.
#[derive(Debug)]
pub struct EnumValue {
    pub name: String,
    pub description: Option<String>,
    pub deprecation: Option<String>,
}

/// A directive the schema declares.
#[derive(Debug)]
pub struct DirectiveDef {
    pub name: String,
    pub description: Option<String>,
    pub locations: Vec<DirectiveLocation>,
    pub args: IndexMap<String, InputValue>,
    pub repeatable: bool,
}

/// Where a directive can be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectiveLocation {
    Query,
    Mutation,
    Subscription,
    Field,
    FragmentDefinition,
    FragmentSpread,
    InlineFragment,
    VariableDefinition,
    Schema,
    Scalar,
    Object,
    FieldDefinition,
    ArgumentDefinition,
    Interface,
    Union,
    Enum,
    EnumValue,
    InputObject,
    InputFieldDefinition,
}

impl DirectiveLocation {
    /// The name of the location in `__DirectiveLocation`: `FIELD`.
    pub fn as_str(self) -> &'static str {
        match self {
            DirectiveLocation::Query => "QUERY",
            DirectiveLocation::Mutation => "MUTATION",
            DirectiveLocation::Subscription => "SUBSCRIPTION",
            DirectiveLocation::Field => "FIELD",
            DirectiveLocation::FragmentDefinition => "FRAGMENT_DEFINITION",
            DirectiveLocation::FragmentSpread => "FRAGMENT_SPREAD",
            DirectiveLocation::InlineFragment => "INLINE_FRAGMENT",
            DirectiveLocation::VariableDefinition => "VARIABLE_DEFINITION",
            DirectiveLocation::Schema => "SCHEMA",
            DirectiveLocation::Scalar => "SCALAR",
            DirectiveLocation::Object => "OBJECT",
            DirectiveLocation::FieldDefinition => "FIELD_DEFINITION",
            DirectiveLocation::ArgumentDefinition => "ARGUMENT_DEFINITION",
            DirectiveLocation::Interface => "INTERFACE",
            DirectiveLocation::Union => "UNION",
            DirectiveLocation::Enum => "ENUM",
            DirectiveLocation::EnumValue => "ENUM_VALUE",
            DirectiveLocation::InputObject => "INPUT_OBJECT",
            DirectiveLocation::InputFieldDefinition => "INPUT_FIELD_DEFINITION",
        }
    }
}

/// The type of a field, an argument or a variable: a named type, or a list or non-null
/// wrapping of one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TypeRef {
    Named(String),
    List(Box<TypeRef>),
    NonNull(Box<TypeRef>),
}

impl TypeRef {
    /// The named type inside the wrappings: `Product` for `[Product!]!`.
    pub fn named(&self) -> &str {
        match self {
            TypeRef::Named(name) => name,
            TypeRef::List(inner) | TypeRef::NonNull(inner) => inner.named(),
        }
    }

    pub fn is_non_null(&self) -> bool {
        matches!(self, TypeRef::NonNull(_))
    }

    /// Whether the type is a list, non-null or not.
    pub fn is_list(&self) -> bool {
        matches!(self.nullable(), TypeRef::List(_))
    }

    /// The type without its outer non-null: `[Product!]` for `[Product!]!`.
    pub fn nullable(&self) -> &TypeRef {
        match self {
            TypeRef::NonNull(inner) => inner,
            other => other,
        }
    }

    /// The type of the items of a list type (`Product!` for `[Product!]!`), `None` when the type
    /// is not a list.
    pub fn item(&self) -> Option<&TypeRef> {
        match self.nullable() {
            TypeRef::List(inner) => Some(inner),
            _ => None,
        }
    }

    /// How deeply lists nest: 0 for `Product!`, 1 for `[Product!]!`.
    pub fn list_depth(&self) -> usize {
        match self {
            TypeRef::Named(_) => 0,
            TypeRef::NonNull(inner) => inner.list_depth(),
            TypeRef::List(inner) => 1 + inner.list_depth(),
        }
    }
}

impl fmt::Display for TypeRef {
    /// The type as GraphQL writes it: `[Product!]!`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeRef::Named(name) => f.write_str(name),
            TypeRef::List(inner) => write!(f, "[{inner}]"),
            TypeRef::NonNull(inner) => write!(f, "{inner}!"),
        }
    }
}

impl Schema {
    /// Parses a schema: the SDL of its types, and the SDL of its introspection types.
    pub fn parse(sdl: &str, introspection: &str) -> Result<Schema, String> {
        sdl::build(sdl, introspection)
    }

    /// The description of the schema (`__Schema.description`).
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// The name of the root type of queries: `QueryRoot`.
    pub fn query_type(&self) -> &str {
        &self.query_type
    }

    /// The name of the root type of mutations: `Mutation`.
    pub fn mutation_type(&self) -> Option<&str> {
        self.mutation_type.as_deref()
    }

    pub fn subscription_type(&self) -> Option<&str> {
        self.subscription_type.as_deref()
    }

    /// A type by name.
    pub fn get(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Every type, in the order introspection lists them.
    pub fn types(&self) -> impl Iterator<Item = &TypeDef> {
        self.types.values()
    }

    /// A field of an object or an interface type.
    pub fn field(&self, type_name: &str, field: &str) -> Option<&Field> {
        self.types.get(type_name)?.fields.get(field)
    }

    /// A field a query may select on a type: one of the type, or `__typename`, or `__schema`
    /// and `__type` on the query root.
    pub fn lookup_field(&self, type_name: &str, field: &str) -> Option<&Field> {
        let [typename, schema, ty] = &self.meta;
        match field {
            "__typename" if self.get(type_name).is_some_and(TypeDef::is_composite) => {
                Some(typename)
            }
            "__schema" if type_name == self.query_type => Some(schema),
            "__type" if type_name == self.query_type => Some(ty),
            _ => self.field(type_name, field),
        }
    }

    /// The type of a field: `ProductConnection!` for `QueryRoot.products`.
    pub fn field_type(&self, type_name: &str, field: &str) -> Option<&TypeRef> {
        Some(&self.field(type_name, field)?.ty)
    }

    /// A directive by name.
    pub fn directive(&self, name: &str) -> Option<&DirectiveDef> {
        self.directives.get(name)
    }

    /// Every directive, in the order introspection lists them.
    pub fn directives(&self) -> impl Iterator<Item = &DirectiveDef> {
        self.directives.values()
    }

    /// The object types a value of a type can be: those of an interface or a union, the type
    /// itself for an object, none for the others.
    pub fn possible_types<'s>(&'s self, name: &'s str) -> &'s [String] {
        match self.types.get(name) {
            Some(def) if def.is_abstract() => &def.possible_types,
            Some(def) if def.kind == Kind::Object => std::slice::from_ref(&def.name),
            _ => &[],
        }
    }

    /// Whether an object of type `object` is a value of type `of`: the same type, or an
    /// interface it implements, or a union it is a member of.
    pub fn is_possible(&self, of: &str, object: &str) -> bool {
        of == object || self.possible_types(of).iter().any(|name| name == object)
    }

    /// The values of an enum, `None` when the type is not an enum.
    pub fn enum_values(&self, name: &str) -> Option<impl Iterator<Item = &EnumValue>> {
        let def = self.types.get(name).filter(|def| def.kind == Kind::Enum)?;
        Some(def.enum_values.values())
    }

    /// Whether a type is a connection: an object with `edges` (whose items have a `node`) and
    /// `pageInfo`, as Relay defines them.
    pub fn is_connection(&self, name: &str) -> bool {
        self.connections.contains_key(name)
    }

    /// The type of the nodes of a connection type: `Product` for `ProductConnection`. `None`
    /// when the type is not a connection.
    pub fn connection_node(&self, name: &str) -> Option<&str> {
        self.connections.get(name).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_schema_of_each_version_is_its_generated_sdl() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for version in ApiVersion::all() {
            let sdl = decompress(version.files().schema);
            let path = root.join(format!("schemas/admin/{version}/admin.graphql"));
            let generated = std::fs::read_to_string(&path).unwrap();
            assert!(
                sdl == generated,
                "schemas/admin/{version}/admin.graphql.br is not admin.graphql: run `mise run admin:schema -- --version {version}`"
            );
        }
    }

    #[test]
    fn reads_the_admin_schema() {
        let schema = admin();
        assert_eq!(schema.query_type(), "QueryRoot");
        assert_eq!(schema.mutation_type(), Some("Mutation"));
        assert_eq!(schema.subscription_type(), None);
        assert_eq!(schema.types().count(), 3699);
        assert_eq!(schema.get("QueryRoot").unwrap().fields.len(), 298);
        assert_eq!(schema.get("Mutation").unwrap().fields.len(), 532);
        // The order of Shopify's introspection, its introspection types last.
        let names: Vec<&str> = schema
            .types()
            .map(|def| def.name.as_str())
            .take(3)
            .collect();
        assert_eq!(names, ["ARN", "AbandonedCheckout", "String"]);
        assert_eq!(schema.types().last().unwrap().name, "__DirectiveLocation");

        let products = schema.field("QueryRoot", "products").unwrap();
        assert_eq!(products.ty.to_string(), "ProductConnection!");
        assert!(
            products
                .description
                .as_deref()
                .unwrap()
                .starts_with("Retrieves a list of")
        );
        assert_eq!(
            products.arg("reverse").unwrap().default_value,
            Some(Value::Bool(false))
        );
        assert_eq!(
            products.arg("sortKey").unwrap().default_literal.as_deref(),
            Some("ID")
        );
        assert_eq!(products.arg("first").unwrap().ty.to_string(), "Int");
        assert!(schema.is_connection("ProductConnection"));
        assert_eq!(schema.connection_node("ProductConnection"), Some("Product"));
        assert!(!schema.is_connection("Product"));

        let node = schema.get("Node").unwrap();
        assert_eq!(node.kind, Kind::Interface);
        assert!(
            schema
                .possible_types("Node")
                .iter()
                .any(|name| name == "Product")
        );
        assert!(schema.is_possible("Node", "Product"));
        assert!(!schema.is_possible("Node", "MoneyV2"));
        let statuses: Vec<&str> = schema
            .enum_values("ProductStatus")
            .unwrap()
            .map(|value| value.name.as_str())
            .collect();
        assert_eq!(statuses, ["ACTIVE", "ARCHIVED", "DRAFT", "UNLISTED"]);
        assert!(schema.get("ProductIdentifierInput").unwrap().one_of);
        assert!(!schema.get("ProductSetInput").unwrap().one_of);

        let idempotent = schema.directive("idempotent").unwrap();
        assert_eq!(idempotent.locations, [DirectiveLocation::Field]);
        assert!(idempotent.args["key"].is_required());
        assert_eq!(
            schema.directive("deprecated").unwrap().args["reason"]
                .default_literal
                .as_deref(),
            Some("\"No longer supported\"")
        );
        assert!(schema.get("__Type").unwrap().field("isOneOf").is_some());
    }

    #[test]
    fn deprecations_keep_their_reason() {
        let schema = admin();
        let deprecated = schema
            .types()
            .flat_map(|def| def.fields.values())
            .find(|field| field.deprecation.is_some())
            .unwrap();
        assert!(!deprecated.deprecation.as_deref().unwrap().is_empty());
    }
}
