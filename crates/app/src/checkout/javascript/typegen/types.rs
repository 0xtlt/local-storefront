//! The `typescript` plugin of graphql-codegen: the `Scalars` map, then one TypeScript type for
//! each type of the schema, in the order of the sorted schema.

use indexmap::IndexMap;

use super::model::{EnumValue, Field, InputValue, Kind, Model, TypeDef, TypeRef};
use super::text::{
    convert_name, indent, strip_trailing_spaces, transform_comment, wrap_with_single_quotes,
};
use super::{Options, Scalar};

/// The lines every output starts with: the helper types the other types use.
pub(super) const HEADER: [&str; 7] = [
    "export type Maybe<T> = T | null;",
    "export type InputMaybe<T> = Maybe<T>;",
    "export type Exact<T extends { [key: string]: unknown }> = { [K in keyof T]: T[K] };",
    "export type MakeOptional<T, K extends keyof T> = Omit<T, K> & { [SubKey in K]?: Maybe<T[SubKey]> };",
    "export type MakeMaybe<T, K extends keyof T> = Omit<T, K> & { [SubKey in K]: Maybe<T[SubKey]> };",
    "export type MakeEmpty<T extends { [key: string]: unknown }, K extends keyof T> = { [_ in K]?: never };",
    "export type Incremental<T> = T | { [P in keyof T]?: P extends ' $fragmentName' | '__typename' ? T[P] : never };",
];

/// The TypeScript type that stands for each scalar of the schema: the five built-in scalars,
/// then the scalars of the schema in order, as the options map them.
pub(super) fn scalar_map(model: &Model, options: &Options) -> IndexMap<String, Scalar> {
    let mut map = IndexMap::new();
    let built_in = |name: &str, input: &str| {
        (
            name.to_string(),
            Scalar {
                input: input.to_string(),
                output: input.to_string(),
            },
        )
    };
    for (name, ty) in [
        ("ID", "string"),
        ("String", "string"),
        ("Boolean", "boolean"),
        ("Int", "number"),
        ("Float", "number"),
    ] {
        let (name, scalar) = built_in(name, ty);
        map.insert(name, scalar);
    }
    for ty in &model.types {
        if !matches!(ty.kind, Kind::Scalar) {
            continue;
        }
        let unused_built_in = ["ID", "Int", "Float"].contains(&ty.name.as_str())
            && !model.used_built_ins.contains(&ty.name);
        if unused_built_in {
            continue;
        }
        if let Some(scalar) = options.scalars.get(&ty.name) {
            map.insert(ty.name.clone(), scalar.clone());
        } else if !map.contains_key(&ty.name) {
            let (name, scalar) = built_in(&ty.name, &options.default_scalar_type);
            map.insert(name, scalar);
        }
    }
    map
}

pub(super) struct Types<'a> {
    pub model: &'a Model,
    pub options: &'a Options,
    pub scalars: &'a IndexMap<String, Scalar>,
}

impl Types<'_> {
    /// The content of the plugin: the `Scalars` type and the types of the schema.
    pub fn content(&self) -> String {
        let mut parts = vec![self.scalars_block()];
        for ty in &self.model.types {
            let part = match &ty.kind {
                Kind::Scalar => String::new(),
                Kind::Object { interfaces, fields } => self.object(ty, interfaces, fields),
                Kind::Interface { fields } => self.interface(ty, fields),
                Kind::Union { members } => self.union(ty, members),
                Kind::Enum { values } => self.enumeration(ty, values),
                Kind::Input { fields, one_of } => self.input(ty, fields, *one_of),
            };
            parts.push(part);
        }
        parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn readonly(&self) -> &'static str {
        if self.options.immutable_types {
            "readonly "
        } else {
            ""
        }
    }

    fn scalars_block(&self) -> String {
        let lines: Vec<String> = self
            .scalars
            .iter()
            .map(|(name, scalar)| {
                let comment = self
                    .model
                    .get(name)
                    .filter(|ty| ty.declared)
                    .and_then(|ty| ty.description.as_deref())
                    .map(|description| transform_comment(description, 1))
                    .unwrap_or_default();
                comment
                    + &indent(
                        &format!(
                            "{name}: {{ input: {}; output: {}; }}",
                            scalar.input, scalar.output
                        ),
                        1,
                    )
            })
            .collect();
        type_declaration(
            "Scalars",
            Some("All built-in and custom scalars, mapped to their actual values"),
            "",
            Some(&lines.join("\n")),
        )
    }

    /// The name of a type of the schema in TypeScript.
    fn name(&self, name: &str) -> String {
        convert_name(name, false)
    }

    /// The type of the schema as TypeScript, with `Maybe` (or `InputMaybe`, in an input type)
    /// around everything that is nullable.
    fn type_string(&self, ty: &TypeRef, input: bool) -> String {
        let wrapper = if input { "InputMaybe" } else { "Maybe" };
        match ty {
            TypeRef::Named(name) => {
                let base = match self.scalars.get(name) {
                    Some(_) if input => format!("Scalars['{name}']['input']"),
                    Some(_) => format!("Scalars['{name}']['output']"),
                    None => self.name(name),
                };
                format!("{wrapper}<{base}>")
            }
            TypeRef::List(inner) => {
                format!(
                    "{wrapper}<{}<{}>>",
                    self.array(),
                    self.type_string(inner, input)
                )
            }
            TypeRef::NonNull(inner) => clear_optional(&self.type_string(inner, input)),
        }
    }

    fn array(&self) -> &'static str {
        if self.options.immutable_types {
            "ReadonlyArray"
        } else {
            "Array"
        }
    }

    fn comment(&self, description: &Option<String>, deprecation: &Option<String>) -> String {
        let mut text = description.clone().unwrap_or_default();
        if let Some(reason) = deprecation {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("@deprecated {reason}"));
        }
        transform_comment(&text, 1)
    }

    fn object(&self, ty: &TypeDef, interfaces: &[String], fields: &[Field]) -> String {
        let mut lines = Vec::new();
        if !self.options.skip_typename {
            let optional = if self.options.non_optional_typename {
                "__typename"
            } else {
                "__typename?"
            };
            lines.push(indent(
                &format!("{}{optional}: '{}';", self.readonly(), ty.name),
                1,
            ));
        }
        lines.extend(fields.iter().map(|field| self.field(field)));
        let interfaces: Vec<String> = interfaces.iter().map(|name| self.name(name)).collect();
        let content = interfaces.join(" & ")
            + if !interfaces.is_empty() && !lines.is_empty() {
                " & "
            } else {
                ""
            };
        let block = type_declaration(
            &self.name(&ty.name),
            ty.description.as_deref(),
            &content,
            Some(&lines.join("\n")),
        );
        let arguments = self.arguments_blocks(ty, fields);
        [block, arguments]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn interface(&self, ty: &TypeDef, fields: &[Field]) -> String {
        let lines: Vec<String> = fields.iter().map(|field| self.field(field)).collect();
        let block = type_declaration(
            &self.name(&ty.name),
            ty.description.as_deref(),
            "",
            Some(&lines.join("\n")),
        );
        let arguments = self.arguments_blocks(ty, fields);
        [block, arguments]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn field(&self, field: &Field) -> String {
        let optional = if field.ty.is_non_null() { "" } else { "?" };
        self.comment(&field.description, &field.deprecation)
            + &indent(
                &format!(
                    "{}{}{optional}: {};",
                    self.readonly(),
                    field.name,
                    self.type_string(&field.ty, false)
                ),
                1,
            )
    }

    /// One `<Type><Field>Args` type for each field that has arguments.
    fn arguments_blocks(&self, ty: &TypeDef, fields: &[Field]) -> String {
        fields
            .iter()
            .filter(|field| !field.args.is_empty())
            .map(|field| {
                let name = format!("{}{}Args", ty.name, convert_name(&field.name, false));
                type_declaration(
                    &self.name(&name),
                    ty.description.as_deref(),
                    "",
                    Some(&self.arguments(&field.args)),
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn arguments(&self, args: &[InputValue]) -> String {
        let lines: Vec<String> = args
            .iter()
            .map(|arg| {
                let ast = VariableType {
                    name: &arg.name,
                    ty: &arg.ty,
                    has_default: arg.has_default,
                };
                indent(
                    &variable_line(&ast, self.scalars, false, self.options, true),
                    1,
                )
            })
            .collect();
        lines.join(";\n") + ";"
    }

    fn input(&self, ty: &TypeDef, fields: &[InputValue], one_of: bool) -> String {
        let name = self.name(&ty.name);
        if one_of {
            let lines: Vec<String> = fields
                .iter()
                .map(|field| self.one_of_field(field, fields))
                .collect();
            let content = format!("\n{}", lines.join("\n  |"));
            return type_declaration(&name, ty.description.as_deref(), &content, None);
        }
        let lines: Vec<String> = fields
            .iter()
            .map(|field| {
                let optional = if !field.ty.is_non_null() || field.has_default {
                    "?"
                } else {
                    ""
                };
                self.comment(&field.description, &field.deprecation)
                    + &indent(
                        &format!(
                            "{}{}{optional}: {};",
                            self.readonly(),
                            field.name,
                            self.type_string(&field.ty, true)
                        ),
                        1,
                    )
            })
            .collect();
        type_declaration(
            &name,
            ty.description.as_deref(),
            "",
            Some(&lines.join("\n")),
        )
    }

    /// A field of a `@oneOf` input: one member of a union in which the other fields are `never`.
    fn one_of_field(&self, field: &InputValue, all: &[InputValue]) -> String {
        let readonly = self.readonly();
        let parts: Vec<String> = all
            .iter()
            .map(|other| {
                if other.name == field.name {
                    format!(
                        "{readonly}{}: {};",
                        field.name,
                        clear_optional(&self.type_string(&field.ty, true))
                    )
                } else {
                    format!("{readonly}{}?: never;", other.name)
                }
            })
            .collect();
        self.comment(&field.description, &field.deprecation)
            + &indent(&format!("{{ {} }}", parts.join(" ")), 1)
    }

    fn union(&self, ty: &TypeDef, members: &[String]) -> String {
        let content = members
            .iter()
            .map(|member| match self.scalars.get(member) {
                Some(scalar) => scalar.output.clone(),
                None => self.name(member),
            })
            .collect::<Vec<_>>()
            .join(" | ");
        type_declaration(
            &self.name(&ty.name),
            ty.description.as_deref(),
            &content,
            None,
        )
    }

    fn enumeration(&self, ty: &TypeDef, values: &[EnumValue]) -> String {
        let name = self.name(&ty.name);
        let options = self.options;
        if options.enums_as_types {
            let lines: Vec<String> = values
                .iter()
                .map(|value| {
                    transform_comment(value.description.as_deref().unwrap_or(""), 1)
                        + &indent(
                            &format!("| {}", wrap_with_single_quotes(&value.name, false)),
                            1,
                        )
                })
                .collect();
            let content = format!("\n{}", lines.join("\n"));
            return type_declaration(&name, ty.description.as_deref(), &content, None);
        }
        if options.enums_as_const {
            let lines: Vec<String> = values
                .iter()
                .map(|value| {
                    transform_comment(value.description.as_deref().unwrap_or(""), 1)
                        + &indent(
                            &format!(
                                "{}: {}",
                                convert_name(&value.name, true),
                                wrap_with_single_quotes(&value.name, false)
                            ),
                            1,
                        )
                })
                .collect();
            let constant = declaration(
                "const",
                &name,
                ty.description.as_deref(),
                "",
                Some(&lines.join(",\n")),
                " as const",
            );
            let alias = format!("export type {name} = typeof {name}[keyof typeof {name}];");
            return [constant, alias].join("\n");
        }
        let lines: Vec<String> = values
            .iter()
            .map(|value| {
                let option = convert_name(&value.name, true);
                let option = if option.starts_with(|c: char| c.is_ascii_digit()) {
                    wrap_with_single_quotes(&option, true)
                } else {
                    option
                };
                self.comment(&value.description, &value.deprecation)
                    + &indent(
                        &format!("{option} = {}", wrap_with_single_quotes(&value.name, true)),
                        1,
                    )
            })
            .collect();
        let kind = if options.const_enums {
            "const enum"
        } else {
            "enum"
        };
        declaration(
            kind,
            &name,
            ty.description.as_deref(),
            "",
            Some(&lines.join(",\n")),
            "",
        )
    }
}

/// `str` without the `Maybe<` (or `InputMaybe<`) around it.
pub(super) fn clear_optional(text: &str) -> String {
    for prefix in ["Maybe<", "InputMaybe<"] {
        if let Some(inner) = text
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix('>'))
        {
            return inner.to_string();
        }
    }
    text.to_string()
}

/// A variable of an operation, or an argument of a field, as the object type that holds it.
pub(super) struct VariableType<'a> {
    pub name: &'a str,
    pub ty: &'a TypeRef,
    pub has_default: bool,
}

/// One `name?: Type` entry of the type of the arguments of a field or the variables of an
/// operation. `coercion` is graphql-codegen's `arrayInputCoercion`: a list accepts a single value
/// too. `schema` tells the arguments of a schema from the variables of an operation, which
/// differ in what a default value does to the type.
pub(super) fn variable_line(
    variable: &VariableType<'_>,
    scalars: &IndexMap<String, Scalar>,
    coercion: bool,
    options: &Options,
    schema: bool,
) -> String {
    let base_name = variable.ty.base();
    let base = match scalars.get(base_name) {
        Some(_) => format!("Scalars['{base_name}']['input']"),
        None => convert_name(base_name, false),
    };
    let array = if options.immutable_types {
        "ReadonlyArray"
    } else {
        "Array"
    };
    let field_type = wrap_ast_type(&base, variable.ty, coercion, array);
    let non_null = variable.ty.is_non_null();
    let optional = variable.has_default || !non_null;
    let type_string = if schema && !variable.has_default && non_null {
        clear_optional_input(&field_type)
    } else {
        field_type
    };
    format!(
        "{}{}: {type_string}",
        variable.name,
        if optional { "?" } else { "" }
    )
}

fn clear_optional_input(text: &str) -> String {
    match text
        .strip_prefix("InputMaybe<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        Some(inner) => inner.to_string(),
        None => text.to_string(),
    }
}

/// The type of a variable: `InputMaybe` around what is nullable, `Array` for a list.
fn wrap_ast_type(base: &str, ty: &TypeRef, coercion: bool, array: &str) -> String {
    match ty {
        TypeRef::NonNull(inner) => {
            clear_optional_input(&wrap_ast_type(base, inner, coercion, array))
        }
        TypeRef::List(inner) => {
            let inner = wrap_ast_type(base, inner, coercion, array);
            let extension = if coercion {
                format!(" | {inner}")
            } else {
                String::new()
            };
            format!("InputMaybe<{array}<{inner}>{extension}>")
        }
        TypeRef::Named(_) => format!("InputMaybe<{base}>"),
    }
}

/// `export type <name> = <content>{ <block> };` with a comment in front.
fn type_declaration(
    name: &str,
    comment: Option<&str>,
    content: &str,
    block: Option<&str>,
) -> String {
    declaration("type", name, comment, content, block, "")
}

/// graphql-codegen's `DeclarationBlock`: `export <kind> <name> = <content>{ <block> }`, with a
/// `;` unless the kind is an `enum`, a line break at the end, and no space in front of a line
/// break. The `=` is for a `type` or a `const`. `after_block` is what follows the braces.
fn declaration(
    kind: &str,
    name: &str,
    comment: Option<&str>,
    content: &str,
    block: Option<&str>,
    after_block: &str,
) -> String {
    let mut result = format!("export {kind} {name} ");
    if matches!(kind, "type" | "const") {
        result.push_str("= ");
    }
    match block {
        Some(block) if !block.is_empty() => {
            result.push_str(&format!("{content}{{\n{block}\n}}{after_block}"));
        }
        _ if !content.is_empty() => result.push_str(content),
        _ => result.push_str(&format!("{{}}{after_block}")),
    }
    let end = if matches!(kind, "enum" | "interface") {
        ""
    } else {
        ";"
    };
    let comment = comment
        .filter(|comment| !comment.is_empty())
        .map(|comment| transform_comment(comment, 0))
        .unwrap_or_default();
    strip_trailing_spaces(&format!("{comment}{result}{end}\n"))
}
