//! Introspection (`__schema`, `__type`), answered from the [`Schema`] through the same executor
//! as the other fields: a [`Resolver`] whose objects are the parts of the schema. Every
//! description, deprecation, default value, `specifiedByURL`, `isOneOf` and `isRepeatable` is
//! there, in the order of Shopify's introspection, so that graphql-js's
//! `getIntrospectionQuery()` (and GraphiQL, graphql-codegen) reads what it reads from Shopify.

use serde_json::Value;

use super::{FieldContext, FieldError, Output, Resolver};
use crate::admin::schema::{
    DirectiveDef, EnumValue, Field, InputValue, Kind, Schema, TypeDef, TypeRef,
};

pub(crate) struct Introspection<'s> {
    pub schema: &'s Schema,
}

/// A part of the schema, as introspection shows it.
#[derive(Clone, Copy)]
pub(crate) enum Meta<'s> {
    Schema,
    /// A named type.
    Named(&'s TypeDef),
    /// A list or non-null type.
    Wrapped(&'s TypeRef),
    Field(&'s Field),
    InputValue(&'s InputValue),
    EnumValue(&'s EnumValue),
    Directive(&'s DirectiveDef),
}

type Answer<'s> = Result<Output<Meta<'s>>, FieldError>;

fn text(value: Option<&str>) -> Output<Meta<'_>> {
    match value {
        Some(text) => Output::Value(Value::String(text.to_string())),
        None => Output::Null,
    }
}

fn boolean<O>(flag: bool) -> Output<O> {
    Output::Value(Value::Bool(flag))
}

impl<'s> Introspection<'s> {
    fn type_ref(&self, ty: &'s TypeRef) -> Output<Meta<'s>> {
        match ty {
            TypeRef::Named(name) => match self.schema.get(name) {
                Some(def) => Output::Object(Meta::Named(def)),
                None => Output::Null,
            },
            wrapped => Output::Object(Meta::Wrapped(wrapped)),
        }
    }

    fn named(&self, name: Option<&str>) -> Output<Meta<'s>> {
        match name.and_then(|name| self.schema.get(name)) {
            Some(def) => Output::Object(Meta::Named(def)),
            None => Output::Null,
        }
    }

    fn input_values(
        &self,
        values: impl Iterator<Item = &'s InputValue>,
        include_deprecated: bool,
    ) -> Output<Meta<'s>> {
        Output::List(
            values
                .filter(|value| include_deprecated || value.deprecation.is_none())
                .map(|value| Output::Object(Meta::InputValue(value)))
                .collect(),
        )
    }

    fn schema_field(&self, field: &str) -> Answer<'s> {
        let schema = self.schema;
        Ok(match field {
            "description" => text(schema.description()),
            "types" => Output::List(
                schema
                    .types()
                    .map(|t| Output::Object(Meta::Named(t)))
                    .collect(),
            ),
            "queryType" => self.named(Some(schema.query_type())),
            "mutationType" => self.named(schema.mutation_type()),
            "subscriptionType" => self.named(schema.subscription_type()),
            "directives" => Output::List(
                schema
                    .directives()
                    .map(|d| Output::Object(Meta::Directive(d)))
                    .collect(),
            ),
            _ => Output::Null,
        })
    }

    fn type_field(&self, def: &'s TypeDef, field: &str, include_deprecated: bool) -> Answer<'s> {
        let has_fields = matches!(def.kind, Kind::Object | Kind::Interface);
        Ok(match field {
            "kind" => Output::Value(def.kind.as_str().into()),
            "name" => text(Some(&def.name)),
            "description" => text(def.description.as_deref()),
            "specifiedByURL" => text(def.specified_by_url.as_deref()),
            "fields" if has_fields => Output::List(
                def.fields
                    .values()
                    .filter(|field| include_deprecated || field.deprecation.is_none())
                    .map(|field| Output::Object(Meta::Field(field)))
                    .collect(),
            ),
            "interfaces" if has_fields => Output::List(
                def.interfaces
                    .iter()
                    .map(|name| self.named(Some(name)))
                    .collect(),
            ),
            "possibleTypes" if def.is_abstract() => Output::List(
                def.possible_types
                    .iter()
                    .map(|name| self.named(Some(name)))
                    .collect(),
            ),
            "enumValues" if def.kind == Kind::Enum => Output::List(
                def.enum_values
                    .values()
                    .filter(|value| include_deprecated || value.deprecation.is_none())
                    .map(|value| Output::Object(Meta::EnumValue(value)))
                    .collect(),
            ),
            "inputFields" if def.kind == Kind::InputObject => {
                self.input_values(def.input_fields.values(), include_deprecated)
            }
            "isOneOf" if def.kind == Kind::InputObject => boolean(def.one_of),
            _ => Output::Null,
        })
    }

    fn wrapped_field(&self, ty: &'s TypeRef, field: &str) -> Answer<'s> {
        Ok(match (ty, field) {
            (TypeRef::NonNull(_), "kind") => Output::Value("NON_NULL".into()),
            (_, "kind") => Output::Value("LIST".into()),
            (TypeRef::NonNull(inner) | TypeRef::List(inner), "ofType") => self.type_ref(inner),
            _ => Output::Null,
        })
    }
}

fn deprecation<O>(field: &str, deprecation: Option<&str>) -> Option<Output<O>> {
    match field {
        "isDeprecated" => Some(boolean(deprecation.is_some())),
        "deprecationReason" => Some(match deprecation {
            Some(reason) => Output::Value(Value::String(reason.to_string())),
            None => Output::Null,
        }),
        _ => None,
    }
}

impl<'s> Resolver for Introspection<'s> {
    type Object = Meta<'s>;

    fn query_root(&mut self) -> Meta<'s> {
        Meta::Schema
    }

    fn mutation_root(&mut self) -> Meta<'s> {
        Meta::Schema
    }

    fn type_name(&self, object: &Meta<'s>) -> String {
        match object {
            Meta::Schema => "__Schema",
            Meta::Named(_) | Meta::Wrapped(_) => "__Type",
            Meta::Field(_) => "__Field",
            Meta::InputValue(_) => "__InputValue",
            Meta::EnumValue(_) => "__EnumValue",
            Meta::Directive(_) => "__Directive",
        }
        .to_string()
    }

    fn field(&mut self, ctx: &FieldContext<'_>, object: &Meta<'s>) -> Answer<'s> {
        let name = ctx.field_name();
        let include_deprecated = ctx.arg_bool("includeDeprecated") == Some(true);
        match *object {
            Meta::Schema => self.schema_field(name),
            Meta::Named(def) => self.type_field(def, name, include_deprecated),
            Meta::Wrapped(ty) => self.wrapped_field(ty, name),
            Meta::Field(field) => Ok(match name {
                "name" => text(Some(&field.name)),
                "description" => text(field.description.as_deref()),
                "args" => self.input_values(field.args.values(), include_deprecated),
                "type" => self.type_ref(&field.ty),
                _ => deprecation(name, field.deprecation.as_deref()).unwrap_or(Output::Null),
            }),
            Meta::InputValue(value) => Ok(match name {
                "name" => text(Some(&value.name)),
                "description" => text(value.description.as_deref()),
                "type" => self.type_ref(&value.ty),
                "defaultValue" => text(value.default_literal.as_deref()),
                _ => deprecation(name, value.deprecation.as_deref()).unwrap_or(Output::Null),
            }),
            Meta::EnumValue(value) => Ok(match name {
                "name" => text(Some(&value.name)),
                "description" => text(value.description.as_deref()),
                _ => deprecation(name, value.deprecation.as_deref()).unwrap_or(Output::Null),
            }),
            Meta::Directive(directive) => Ok(match name {
                "name" => text(Some(&directive.name)),
                "description" => text(directive.description.as_deref()),
                "isRepeatable" => boolean(directive.repeatable),
                "locations" => Output::List(
                    directive
                        .locations
                        .iter()
                        .map(|location| Output::Value(location.as_str().into()))
                        .collect(),
                ),
                "args" => self.input_values(directive.args.values(), include_deprecated),
                _ => Output::Null,
            }),
        }
    }
}
