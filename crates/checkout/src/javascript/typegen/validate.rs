//! The checks graphql-codegen makes on the documents before it writes anything, against the
//! schema: an operation that is not valid is an error and no file is written.
//!
//! These are the rules of graphql-js that graphql-codegen runs (it leaves out `NoUnusedFragments`,
//! `NoUnusedVariables` and `KnownDirectives`). Not checked: `UniqueInputFieldNames` (the parser
//! merges the duplicate keys of an object), and, for a directive other than `@skip` and
//! `@include`, its arguments and how many times it is used. A document with `@defer`, `@stream` or
//! an introspection field is refused, though graphql-codegen takes it (it writes other types for
//! it).

use std::collections::{HashMap, HashSet};

use graphql_parser::query as q;
use indexmap::IndexMap;

use super::model::{Kind, Model, TypeRef};
use super::operations::{Definition, Source};

type Value = graphql_parser::query::Value<'static, String>;

pub(super) fn validate(model: &Model, sources: &[Source]) -> Result<(), String> {
    let mut errors = Vec::new();
    let fragments: HashMap<&str, &q::FragmentDefinition<'static, String>> = sources
        .iter()
        .filter_map(|source| match &source.definition {
            Definition::Fragment(fragment) => Some((fragment.name.as_str(), fragment)),
            Definition::Operation(_) => None,
        })
        .collect();
    let operations = sources
        .iter()
        .filter(|source| matches!(source.definition, Definition::Operation(_)))
        .count();
    for source in sources {
        let mut check = Check {
            model,
            fragments: &fragments,
            path: &source.path,
            errors: &mut errors,
        };
        match &source.definition {
            Definition::Operation(operation) => check.operation(operation, operations),
            Definition::Fragment(fragment) => check.fragment(fragment),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the documents do not match the schema:\n{}",
            errors.join("\n")
        ))
    }
}

/// A field of a selection, with the type it is selected on and the type it has in the schema.
struct Entry {
    parent: String,
    field: q::Field<'static, String>,
    ty: Option<TypeRef>,
}

struct Check<'a> {
    model: &'a Model,
    fragments: &'a HashMap<&'a str, &'a q::FragmentDefinition<'static, String>>,
    path: &'a str,
    errors: &'a mut Vec<String>,
}

impl Check<'_> {
    fn error(&mut self, position: graphql_parser::Pos, message: String) {
        self.errors.push(format!(
            "  {}:{}:{}: {message}",
            self.path, position.line, position.column
        ));
    }

    fn operation(
        &mut self,
        operation: &q::OperationDefinition<'static, String>,
        operations: usize,
    ) {
        let (kind, name, variables, set, position) = match operation {
            q::OperationDefinition::SelectionSet(set) => {
                // The line of a selection set is offset by the number of its document.
                let start = set.span.0;
                let position = graphql_parser::Pos {
                    line: start.line % 1_000_000,
                    column: start.column,
                };
                ("query", None, &[][..], set, position)
            }
            q::OperationDefinition::Query(query) => (
                "query",
                query.name.as_deref(),
                &query.variable_definitions[..],
                &query.selection_set,
                query.position,
            ),
            q::OperationDefinition::Mutation(mutation) => (
                "mutation",
                mutation.name.as_deref(),
                &mutation.variable_definitions[..],
                &mutation.selection_set,
                mutation.position,
            ),
            q::OperationDefinition::Subscription(subscription) => (
                "subscription",
                subscription.name.as_deref(),
                &subscription.variable_definitions[..],
                &subscription.selection_set,
                subscription.position,
            ),
        };
        if name.is_none() && operations > 1 {
            self.error(
                position,
                "This anonymous operation must be the only defined operation.".to_string(),
            );
        }
        let Some(root) = self.model.root(kind) else {
            self.error(
                position,
                format!("The schema is not configured for {kind}s."),
            );
            return;
        };
        let mut defined = HashSet::new();
        for variable in variables {
            if !defined.insert(variable.name.as_str()) {
                self.error(
                    variable.position,
                    format!(
                        "There can be only one variable named \"${}\".",
                        variable.name
                    ),
                );
            }
            let base = base_name(&variable.var_type);
            match self.model.get(base).map(|ty| &ty.kind) {
                None => self.error(variable.position, format!("Unknown type \"{base}\".")),
                Some(Kind::Scalar | Kind::Enum { .. } | Kind::Input { .. }) => {}
                Some(_) => self.error(
                    variable.position,
                    format!(
                        "Variable \"${}\" cannot be non-input type \"{base}\".",
                        variable.name
                    ),
                ),
            }
        }
        self.selection_set(root, set);
        if kind == "subscription" {
            let mut fields = IndexMap::new();
            self.collect_fields(root, &set.items, &mut fields, &mut HashSet::new());
            if fields.len() != 1 {
                self.error(
                    position,
                    "A subscription must select only one top level field.".to_string(),
                );
            }
        }
        let mut usages = Vec::new();
        self.usages(root, &set.items, &mut usages, &mut HashSet::new());
        for usage in usages {
            let Some(definition) = variables
                .iter()
                .find(|variable| variable.name == usage.name)
            else {
                self.error(
                    usage.position,
                    format!("Variable \"${}\" is not defined.", usage.name),
                );
                continue;
            };
            let Some(expected) = &usage.expected else {
                continue;
            };
            let variable_type = TypeRef::from_ast(&definition.var_type);
            let has_default = definition
                .default_value
                .as_ref()
                .is_some_and(|value| !matches!(value, Value::Null));
            if !allowed_position(
                &variable_type,
                has_default,
                expected,
                usage.location_default,
            ) {
                self.error(
                    usage.position,
                    format!(
                        "Variable \"${}\" of type \"{}\" used in position expecting type \"{}\".",
                        usage.name,
                        type_text(&variable_type),
                        type_text(expected)
                    ),
                );
            }
        }
    }

    fn fragment(&mut self, fragment: &q::FragmentDefinition<'static, String>) {
        let q::TypeCondition::On(on) = &fragment.type_condition;
        if !self.model.get(on).is_some() {
            self.error(fragment.position, format!("Unknown type \"{on}\"."));
            return;
        }
        if !self.model.is_composite(on) {
            self.error(
                fragment.position,
                format!(
                    "Fragment \"{}\" cannot condition on non composite type \"{on}\".",
                    fragment.name
                ),
            );
            return;
        }
        if self.reaches(
            &fragment.name,
            &fragment.selection_set.items,
            &mut HashSet::new(),
        ) {
            self.error(
                fragment.position,
                format!(
                    "Cannot spread fragment \"{}\" within itself.",
                    fragment.name
                ),
            );
            return;
        }
        self.selection_set(on, &fragment.selection_set);
    }

    /// Whether the selections spread the fragment `target`, directly or through other fragments.
    fn reaches(
        &self,
        target: &str,
        selections: &[q::Selection<'static, String>],
        seen: &mut HashSet<String>,
    ) -> bool {
        selections.iter().any(|selection| match selection {
            q::Selection::Field(field) => self.reaches(target, &field.selection_set.items, seen),
            q::Selection::InlineFragment(inline) => {
                self.reaches(target, &inline.selection_set.items, seen)
            }
            q::Selection::FragmentSpread(spread) => {
                if spread.fragment_name == target {
                    return true;
                }
                if !seen.insert(spread.fragment_name.clone()) {
                    return false;
                }
                self.fragments
                    .get(spread.fragment_name.as_str())
                    .is_some_and(|fragment| {
                        self.reaches(target, &fragment.selection_set.items, seen)
                    })
            }
        })
    }

    fn selection_set(&mut self, parent: &str, set: &q::SelectionSet<'static, String>) {
        self.overlapping_fields(parent, set);
        for selection in &set.items {
            match selection {
                q::Selection::Field(field) => self.field(parent, field),
                q::Selection::InlineFragment(inline) => {
                    let on = match &inline.type_condition {
                        Some(q::TypeCondition::On(on)) => on.as_str(),
                        None => parent,
                    };
                    if self.model.get(on).is_none() {
                        self.error(inline.position, format!("Unknown type \"{on}\"."));
                        continue;
                    }
                    if !self.model.is_composite(on) {
                        self.error(
                            inline.position,
                            format!("Fragment cannot condition on non composite type \"{on}\"."),
                        );
                        continue;
                    }
                    if !self.overlap(parent, on) {
                        self.error(
                            inline.position,
                            format!("Fragment cannot be spread here as objects of type \"{parent}\" can never be of type \"{on}\"."),
                        );
                    }
                    self.directives(&inline.directives, inline.position);
                    self.selection_set(on, &inline.selection_set);
                }
                q::Selection::FragmentSpread(spread) => {
                    let Some(fragment) = self.fragments.get(spread.fragment_name.as_str()) else {
                        self.error(
                            spread.position,
                            format!("Unknown fragment \"{}\".", spread.fragment_name),
                        );
                        continue;
                    };
                    let q::TypeCondition::On(on) = &fragment.type_condition;
                    if self.model.get(on).is_some() && !self.overlap(parent, on) {
                        self.error(
                            spread.position,
                            format!(
                                "Fragment \"{}\" cannot be spread here as objects of type \"{parent}\" can never be of type \"{on}\".",
                                spread.fragment_name
                            ),
                        );
                    }
                    self.directives(&spread.directives, spread.position);
                }
            }
        }
    }

    /// `OverlappingFieldsCanBeMerged`: two fields with one response name in a selection (also
    /// through fragments) select the same field with the same arguments, or are on different
    /// object types; their types agree; what they select merges the same way.
    fn overlapping_fields(&mut self, parent: &str, set: &q::SelectionSet<'static, String>) {
        let mut fields = IndexMap::new();
        self.collect_fields(parent, &set.items, &mut fields, &mut HashSet::new());
        for (name, entries) in &fields {
            for (i, first) in entries.iter().enumerate() {
                for second in &entries[i + 1..] {
                    if let Some(reason) = self.conflict(first, second, false) {
                        self.error(
                            first.field.position,
                            format!(
                                "Fields \"{name}\" conflict because {reason}. Use different aliases on the fields to fetch both if this was intentional."
                            ),
                        );
                    }
                }
            }
        }
    }

    fn collect_fields(
        &self,
        parent: &str,
        selections: &[q::Selection<'static, String>],
        fields: &mut IndexMap<String, Vec<Entry>>,
        spreading: &mut HashSet<String>,
    ) {
        for selection in selections {
            match selection {
                q::Selection::Field(field) => {
                    let name = field.alias.clone().unwrap_or_else(|| field.name.clone());
                    let ty = self
                        .model
                        .field(parent, &field.name)
                        .map(|schema_field| schema_field.ty.clone());
                    fields.entry(name).or_default().push(Entry {
                        parent: parent.to_string(),
                        field: field.clone(),
                        ty,
                    });
                }
                q::Selection::InlineFragment(inline) => {
                    let on = match &inline.type_condition {
                        Some(q::TypeCondition::On(on)) => on.as_str(),
                        None => parent,
                    };
                    self.collect_fields(on, &inline.selection_set.items, fields, spreading);
                }
                q::Selection::FragmentSpread(spread) => {
                    let Some(fragment) = self.fragments.get(spread.fragment_name.as_str()) else {
                        continue;
                    };
                    if !spreading.insert(spread.fragment_name.clone()) {
                        continue;
                    }
                    let q::TypeCondition::On(on) = &fragment.type_condition;
                    self.collect_fields(on, &fragment.selection_set.items, fields, spreading);
                    spreading.remove(&spread.fragment_name);
                }
            }
        }
    }

    /// Why two fields of one response name cannot be merged, if they cannot.
    fn conflict(&self, first: &Entry, second: &Entry, parents_exclusive: bool) -> Option<String> {
        let is_object = |name: &str| {
            matches!(
                self.model.get(name).map(|ty| &ty.kind),
                Some(Kind::Object { .. })
            )
        };
        let exclusive = parents_exclusive
            || (first.parent != second.parent
                && is_object(&first.parent)
                && is_object(&second.parent));
        if !exclusive {
            if first.field.name != second.field.name {
                return Some(format!(
                    "\"{}\" and \"{}\" are different fields",
                    first.field.name, second.field.name
                ));
            }
            let same_arguments = |a: &q::Field<'static, String>, b: &q::Field<'static, String>| {
                a.arguments.len() == b.arguments.len()
                    && a.arguments.iter().all(|(name, value)| {
                        b.arguments
                            .iter()
                            .any(|(other, v)| other == name && v == value)
                    })
            };
            if !same_arguments(&first.field, &second.field) {
                return Some("they have differing arguments".to_string());
            }
        }
        let (Some(a_ty), Some(b_ty)) = (&first.ty, &second.ty) else {
            return None;
        };
        if self.types_conflict(a_ty, b_ty) {
            return Some(format!(
                "they return conflicting types \"{}\" and \"{}\"",
                type_text(a_ty),
                type_text(b_ty)
            ));
        }
        let (a, b) = (&first.field.selection_set, &second.field.selection_set);
        if a.items.is_empty() || b.items.is_empty() {
            return None;
        }
        let (mut left, mut right) = (IndexMap::new(), IndexMap::new());
        self.collect_fields(a_ty.base(), &a.items, &mut left, &mut HashSet::new());
        self.collect_fields(b_ty.base(), &b.items, &mut right, &mut HashSet::new());
        for (name, firsts) in &left {
            let Some(seconds) = right.get(name) else {
                continue;
            };
            for x in firsts {
                for y in seconds {
                    if let Some(reason) = self.conflict(x, y, exclusive) {
                        return Some(format!("subfields \"{name}\" conflict because {reason}"));
                    }
                }
            }
        }
        None
    }

    fn types_conflict(&self, a: &TypeRef, b: &TypeRef) -> bool {
        match (a, b) {
            (TypeRef::List(a), TypeRef::List(b)) | (TypeRef::NonNull(a), TypeRef::NonNull(b)) => {
                self.types_conflict(a, b)
            }
            (TypeRef::List(_), _) | (_, TypeRef::List(_)) => true,
            (TypeRef::NonNull(_), _) | (_, TypeRef::NonNull(_)) => true,
            (TypeRef::Named(a), TypeRef::Named(b)) => {
                let leaf = |name: &str| {
                    matches!(
                        self.model.get(name).map(|ty| &ty.kind),
                        Some(Kind::Scalar | Kind::Enum { .. })
                    )
                };
                (leaf(a) || leaf(b)) && a != b
            }
        }
    }

    fn overlap(&self, a: &str, b: &str) -> bool {
        if a == b {
            return true;
        }
        let (a, b) = (self.model.possible_types(a), self.model.possible_types(b));
        a.iter().any(|name| b.contains(name))
    }

    fn field(&mut self, parent: &str, field: &q::Field<'static, String>) {
        let has_selection = !field.selection_set.items.is_empty();
        if field.name == "__typename" {
            if has_selection {
                self.error(
                    field.position,
                    "Field \"__typename\" must not have a selection since type \"String!\" has no subfields."
                        .to_string(),
                );
            }
            return;
        }
        if field.name.starts_with("__") {
            self.error(
                field.position,
                format!(
                    "The introspection field \"{}\" is not supported.",
                    field.name
                ),
            );
            return;
        }
        let Some(schema_field) = self.model.field(parent, &field.name) else {
            self.error(
                field.position,
                format!(
                    "Cannot query field \"{}\" on type \"{parent}\".",
                    field.name
                ),
            );
            return;
        };
        let base = schema_field.ty.base();
        let composite = self.model.is_composite(base);
        if composite && !has_selection {
            self.error(
                field.position,
                format!(
                    "Field \"{}\" of type \"{base}\" must have a selection of subfields.",
                    field.name
                ),
            );
        } else if !composite && has_selection {
            self.error(
                field.position,
                format!(
                    "Field \"{}\" must not have a selection since type \"{base}\" has no subfields.",
                    field.name
                ),
            );
        }
        let mut argument_names = HashSet::new();
        for (name, value) in &field.arguments {
            if !argument_names.insert(name.as_str()) {
                self.error(
                    field.position,
                    format!("There can be only one argument named \"{name}\"."),
                );
            }
            match schema_field.args.iter().find(|arg| arg.name == *name) {
                None => self.error(
                    field.position,
                    format!(
                        "Unknown argument \"{name}\" on field \"{parent}.{}\".",
                        field.name
                    ),
                ),
                Some(arg) => self.check_value(value, &arg.ty, field.position),
            }
        }
        for arg in &schema_field.args {
            let given = field.arguments.iter().any(|(name, _)| *name == arg.name);
            let required = arg.ty.is_non_null() && !arg.has_default;
            let null = field
                .arguments
                .iter()
                .any(|(name, value)| *name == arg.name && matches!(value, Value::Null));
            if required && (!given || null) {
                self.error(
                    field.position,
                    format!(
                        "Field \"{}\" argument \"{}\" of type \"{}\" is required, but it was not provided.",
                        field.name,
                        arg.name,
                        type_text(&arg.ty)
                    ),
                );
            }
        }
        self.directives(&field.directives, field.position);
        if composite && has_selection {
            self.selection_set(base, &field.selection_set);
        }
    }

    fn directives(
        &mut self,
        directives: &[q::Directive<'static, String>],
        position: graphql_parser::Pos,
    ) {
        let mut seen = HashSet::new();
        for directive in directives {
            let conditional = directive.name == "skip" || directive.name == "include";
            if conditional && !seen.insert(directive.name.as_str()) {
                self.error(
                    position,
                    format!(
                        "The directive \"@{}\" can only be used once at this location.",
                        directive.name
                    ),
                );
            }
            if conditional {
                for (name, _) in &directive.arguments {
                    if name != "if" {
                        self.error(
                            position,
                            format!(
                                "Unknown argument \"{name}\" on directive \"@{}\".",
                                directive.name
                            ),
                        );
                    }
                }
            }
            if (directive.name == "skip" || directive.name == "include")
                && !directive.arguments.iter().any(|(name, _)| name == "if")
            {
                self.error(
                    position,
                    format!(
                        "Directive \"@{}\" argument \"if\" of type \"Boolean!\" is required, but it was not provided.",
                        directive.name
                    ),
                );
            }
            if directive.name == "skip" || directive.name == "include" {
                let boolean = TypeRef::NonNull(Box::new(TypeRef::Named("Boolean".to_string())));
                for (name, value) in &directive.arguments {
                    if name == "if" {
                        self.check_value(value, &boolean, position);
                    }
                }
            }
            if directive.name == "defer" || directive.name == "stream" {
                self.error(
                    position,
                    format!("The directive \"@{}\" is not supported.", directive.name),
                );
            }
        }
    }

    /// The variables the selections use, with the type each is used as, and the fragments they
    /// spread.
    fn usages(
        &self,
        parent: &str,
        selections: &[q::Selection<'static, String>],
        out: &mut Vec<Usage>,
        visited: &mut HashSet<String>,
    ) {
        for selection in selections {
            match selection {
                q::Selection::Field(field) => {
                    let schema_field = self.model.field(parent, &field.name);
                    for (name, value) in &field.arguments {
                        let arg = schema_field.and_then(|schema_field| {
                            schema_field.args.iter().find(|arg| arg.name == *name)
                        });
                        self.value_usages(
                            value,
                            arg.map(|arg| (&arg.ty, arg.has_default)),
                            field.position,
                            out,
                        );
                    }
                    self.directive_usages(&field.directives, field.position, out);
                    if let Some(schema_field) = schema_field {
                        self.usages(
                            schema_field.ty.base(),
                            &field.selection_set.items,
                            out,
                            visited,
                        );
                    }
                }
                q::Selection::InlineFragment(inline) => {
                    self.directive_usages(&inline.directives, inline.position, out);
                    let on = match &inline.type_condition {
                        Some(q::TypeCondition::On(on)) => on.as_str(),
                        None => parent,
                    };
                    self.usages(on, &inline.selection_set.items, out, visited);
                }
                q::Selection::FragmentSpread(spread) => {
                    self.directive_usages(&spread.directives, spread.position, out);
                    if visited.insert(spread.fragment_name.clone())
                        && let Some(fragment) = self.fragments.get(spread.fragment_name.as_str())
                    {
                        let q::TypeCondition::On(on) = &fragment.type_condition;
                        self.usages(on, &fragment.selection_set.items, out, visited);
                    }
                }
            }
        }
    }

    fn directive_usages(
        &self,
        directives: &[q::Directive<'static, String>],
        position: graphql_parser::Pos,
        out: &mut Vec<Usage>,
    ) {
        let boolean = TypeRef::NonNull(Box::new(TypeRef::Named("Boolean".to_string())));
        for directive in directives {
            for (name, value) in &directive.arguments {
                let known =
                    (directive.name == "skip" || directive.name == "include") && name == "if";
                self.value_usages(value, known.then_some((&boolean, false)), position, out);
            }
        }
    }

    /// The variables in a value, with the type each is used as: `expected` is the type (and
    /// whether it has a default) the value is given as.
    fn value_usages(
        &self,
        value: &Value,
        expected: Option<(&TypeRef, bool)>,
        position: graphql_parser::Pos,
        out: &mut Vec<Usage>,
    ) {
        match value {
            Value::Variable(name) => out.push(Usage {
                name: name.clone(),
                expected: expected.map(|(ty, _)| ty.clone()),
                location_default: expected.is_some_and(|(_, default)| default),
                position,
            }),
            Value::List(items) => {
                let item = expected.and_then(|(ty, _)| match nullable(ty) {
                    TypeRef::List(inner) => Some(&**inner),
                    _ => None,
                });
                for value in items {
                    self.value_usages(value, item.map(|ty| (ty, false)), position, out);
                }
            }
            Value::Object(fields) => {
                let input = expected.and_then(|(ty, _)| {
                    match self.model.get(ty.base()).map(|ty| &ty.kind) {
                        Some(Kind::Input { fields, .. }) => Some(fields),
                        _ => None,
                    }
                });
                for (name, value) in fields {
                    let field =
                        input.and_then(|fields| fields.iter().find(|field| field.name == *name));
                    self.value_usages(
                        value,
                        field.map(|field| (&field.ty, field.has_default)),
                        position,
                        out,
                    );
                }
            }
            _ => {}
        }
    }

    /// `ValuesOfCorrectType`: a literal is a value of the type it is given as. A variable is
    /// not checked here.
    fn check_value(&mut self, value: &Value, ty: &TypeRef, position: graphql_parser::Pos) {
        if let Some(message) = self.value_error(value, ty) {
            self.error(position, message);
        }
    }

    fn value_error(&self, value: &Value, ty: &TypeRef) -> Option<String> {
        let expected = || {
            format!(
                "Expected value of type \"{}\", found {}.",
                type_text(ty),
                value_text(value)
            )
        };
        match (value, ty) {
            (Value::Variable(_), _) => None,
            (Value::Null, TypeRef::NonNull(_)) => Some(expected()),
            (Value::Null, _) => None,
            (_, TypeRef::NonNull(inner)) => self.value_error(value, inner),
            (_, TypeRef::List(inner)) => match value {
                Value::List(items) => items.iter().find_map(|item| self.value_error(item, inner)),
                _ => self.value_error(value, inner),
            },
            (_, TypeRef::Named(name)) => {
                match self.model.get(name).map(|ty| &ty.kind) {
                    Some(Kind::Scalar) => {
                        let fits = match name.as_str() {
                            "Int" => {
                                matches!(value, Value::Int(number) if number.as_i64().is_some_and(|n| i32::try_from(n).is_ok()))
                            }
                            "Float" => matches!(value, Value::Int(_) | Value::Float(_)),
                            "String" => matches!(value, Value::String(_)),
                            "Boolean" => matches!(value, Value::Boolean(_)),
                            "ID" => matches!(value, Value::String(_) | Value::Int(_)),
                            _ => true,
                        };
                        (!fits).then(expected)
                    }
                    Some(Kind::Enum { values }) => match value {
                        Value::Enum(found) if values.iter().any(|v| v.name == *found) => None,
                        Value::Enum(found) => Some(format!(
                            "Value \"{found}\" does not exist in \"{name}\" enum."
                        )),
                        _ => Some(expected()),
                    },
                    Some(Kind::Input { fields, one_of }) => match value {
                        Value::Object(given) => {
                            for (key, value) in given {
                                match fields.iter().find(|field| field.name == *key) {
                                    None => {
                                        return Some(format!(
                                            "Field \"{key}\" is not defined by type \"{name}\"."
                                        ));
                                    }
                                    Some(field) => {
                                        if let Some(error) = self.value_error(value, &field.ty) {
                                            return Some(error);
                                        }
                                    }
                                }
                            }
                            if *one_of {
                                let non_null = given
                                    .values()
                                    .filter(|value| !matches!(value, Value::Null))
                                    .count();
                                if given.len() != 1 || non_null != 1 {
                                    return Some(format!(
                                        "OneOf Input Object \"{name}\" must specify exactly one key."
                                    ));
                                }
                                return None;
                            }
                            fields
                            .iter()
                            .find(|field| {
                                field.ty.is_non_null() && !field.has_default && !given.contains_key(&field.name)
                            })
                            .map(|field| {
                                format!(
                                    "Field \"{name}.{}\" of required type \"{}\" was not provided.",
                                    field.name,
                                    type_text(&field.ty)
                                )
                            })
                        }
                        _ => Some(expected()),
                    },
                    _ => None,
                }
            }
        }
    }
}

/// A variable used somewhere: the type that place takes, and whether it has a default value.
struct Usage {
    name: String,
    expected: Option<TypeRef>,
    location_default: bool,
    position: graphql_parser::Pos,
}

fn nullable(ty: &TypeRef) -> &TypeRef {
    match ty {
        TypeRef::NonNull(inner) => inner,
        other => other,
    }
}

/// `allowedVariableUsage`: whether a variable of `variable` type (with or without a default) can
/// be used where `location` is expected.
fn allowed_position(
    variable: &TypeRef,
    variable_default: bool,
    location: &TypeRef,
    location_default: bool,
) -> bool {
    if let TypeRef::NonNull(inner) = location
        && !variable.is_non_null()
    {
        if !variable_default && !location_default {
            return false;
        }
        return is_sub_type_of(variable, inner);
    }
    is_sub_type_of(variable, location)
}

fn is_sub_type_of(sub: &TypeRef, sup: &TypeRef) -> bool {
    match (sub, sup) {
        (a, b) if a == b => true,
        (TypeRef::NonNull(a), TypeRef::NonNull(b)) => is_sub_type_of(a, b),
        (TypeRef::NonNull(a), b) => is_sub_type_of(a, b),
        (_, TypeRef::NonNull(_)) => false,
        (TypeRef::List(a), TypeRef::List(b)) => is_sub_type_of(a, b),
        _ => false,
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Variable(name) => format!("${name}"),
        Value::Int(number) => format!("{}", number.as_i64().unwrap_or_default()),
        Value::Float(number) => number.to_string(),
        Value::String(text) => format!("{text:?}"),
        Value::Boolean(flag) => flag.to_string(),
        Value::Null => "null".to_string(),
        Value::Enum(name) => name.clone(),
        Value::List(_) => "a list".to_string(),
        Value::Object(_) => "an object".to_string(),
    }
}

fn base_name<'a>(ty: &'a q::Type<'static, String>) -> &'a str {
    match ty {
        q::Type::NamedType(name) => name,
        q::Type::ListType(inner) | q::Type::NonNullType(inner) => base_name(inner),
    }
}

fn type_text(ty: &super::model::TypeRef) -> String {
    match ty {
        super::model::TypeRef::Named(name) => name.clone(),
        super::model::TypeRef::List(inner) => format!("[{}]", type_text(inner)),
        super::model::TypeRef::NonNull(inner) => format!("{}!", type_text(inner)),
    }
}
