//! The validation of a document against the schema: the rules of section 5 of the GraphQL
//! spec, as graphql-ruby (2.6) implements them, with its messages, `extensions.code` and
//! `path`s, so that a query Shopify refuses is refused the same way.
//!
//! graphql-ruby runs its rules as one walk of the document, each rule a hook on a kind of node
//! (`StaticValidation::ALL_RULES`, in that order): the errors of a node come in the order of the
//! rules, some rules stop the walk below a node they refuse (an unknown field is not checked
//! further), and the rules that need the whole document (unused fragments, unused variables)
//! report at the end, in the reverse order of the list. This walk does the same.
//!
//! Where graphql-ruby keeps a legacy behaviour behind a setting whose value on Shopify is not
//! known (`allow_legacy_invalid_*`), the rule follows the spec: a union field without a
//! selection is refused, two fields that cannot be merged are refused, and the variables used in
//! a fragment are checked against each operation that spreads it.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde_json::Value;

use super::ast::*;
use super::error::Error;
use super::parse::MAX_DEPTH;
use super::{input, print};
use crate::admin::schema::{
    DirectiveLocation, Field as FieldDef, InputValue, Kind, Schema, TypeDef, TypeRef,
};

/// Every error of a document, in the order graphql-ruby reports them.
pub fn validate(schema: &Schema, document: &Document<'_>) -> Vec<Error> {
    let mut validator = Validator {
        schema,
        document,
        errors: Vec::new(),
        path: Vec::new(),
        undefined_directives: HashMap::new(),
        operation_names: IndexMap::new(),
        fragment_names: IndexMap::new(),
        usages: IndexMap::new(),
        current: 0,
        in_fragment: false,
        spreads: Vec::new(),
        declared: HashMap::new(),
        schema_definitions: Vec::new(),
        reported_conflicts: HashSet::new(),
    };
    validator.document();
    validator.errors
}

/// What a definition (an operation or a fragment) uses, for the rules that check it at the end.
#[derive(Default)]
struct Usage<'a, 'q> {
    /// The fragment spreads inside, in order: name, where, path.
    spreads: Vec<(&'q str, Pos, Vec<Value>)>,
    /// Its variables, in the order they are declared then first used.
    variables: IndexMap<&'q str, VariableUse>,
    /// For a fragment: the variables arguments are given, checked against the operations.
    arguments: Vec<ArgumentUse<'a, 'q>>,
}

#[derive(Clone, Default)]
struct VariableUse {
    declared: bool,
    /// The path where the operation declares it.
    declared_path: Vec<Value>,
    /// The last place it is used: where, the path, and the definition (its index in the
    /// document) that uses it.
    used: Option<(Pos, Vec<Value>, usize)>,
}

struct ArgumentUse<'a, 'q> {
    argument: &'a Argument<'q>,
    definition: &'a InputValue,
    variable: &'q str,
    list_depth: usize,
    path: Vec<Value>,
}

/// What holds an argument: a field, a directive, or an input object value.
#[derive(Clone, Copy)]
enum Owner<'a> {
    /// The field's response key and its definition, if it has one.
    Field(&'a str, Option<&'a FieldDef>, Pos),
    Directive(&'a str, Option<&'a crate::admin::schema::DirectiveDef>, Pos),
    /// The input object type, if the value is one, and where the value is.
    InputObject(Option<&'a TypeDef>, Pos),
}

impl<'a> Owner<'a> {
    /// The class of the node, as graphql-ruby names it in its messages.
    fn kind(self) -> &'static str {
        match self {
            Owner::Field(..) => "Field",
            Owner::Directive(..) => "Directive",
            Owner::InputObject(..) => "InputObject",
        }
    }

    /// Whether the owner has a definition, which says which arguments it takes.
    fn is_known(self) -> bool {
        match self {
            Owner::Field(_, def, _) => def.is_some(),
            Owner::Directive(_, def, _) => def.is_some(),
            Owner::InputObject(def, _) => def.is_some(),
        }
    }

    fn argument(self, name: &str) -> Option<&'a InputValue> {
        match self {
            Owner::Field(_, def, _) => def?.arg(name),
            Owner::Directive(_, def, _) => def?.args.get(name),
            Owner::InputObject(def, _) => def?.input_field(name),
        }
    }

    /// The name of the owner in messages: the field's alias or name, the directive's name, the
    /// input object type's name.
    fn name(self) -> &'a str {
        match self {
            Owner::Field(name, ..) | Owner::Directive(name, ..) => name,
            Owner::InputObject(def, _) => def.map_or("", |def| def.name.as_str()),
        }
    }

    fn pos(self) -> Pos {
        match self {
            Owner::Field(.., pos) | Owner::Directive(.., pos) | Owner::InputObject(_, pos) => pos,
        }
    }
}

struct Validator<'a, 'q> {
    schema: &'a Schema,
    document: &'a Document<'q>,
    errors: Vec<Error>,
    /// The path of the current node in the document, as graphql-ruby writes it:
    /// `["query Products", "products", "edges", "... on Product", "first"]`.
    path: Vec<Value>,
    /// The error of each directive that is not defined: one error lists every place.
    undefined_directives: HashMap<&'q str, usize>,
    operation_names: IndexMap<Option<&'q str>, Vec<Pos>>,
    fragment_names: IndexMap<&'q str, Vec<Pos>>,
    /// What each visited definition uses, by its index in the document.
    usages: IndexMap<usize, Usage<'a, 'q>>,
    /// The index of the definition being visited.
    current: usize,
    in_fragment: bool,
    /// The named spreads, with the type they are spread in, checked once every fragment is known.
    spreads: Vec<(&'a FragmentSpread<'q>, Option<&'a TypeDef>, Vec<Value>)>,
    /// The variables the operation being visited declares.
    declared: HashMap<&'q str, &'a VariableDefinition<'q>>,
    schema_definitions: Vec<Pos>,
    /// The pairs of fields whose conflict was reported, so that a nested selection checked again
    /// does not report it twice.
    reported_conflicts: HashSet<(&'static str, Pos, Pos)>,
}

fn label(kind: OperationKind, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("{} {name}", kind.as_str()),
        None => kind.as_str().to_string(),
    }
}

impl<'a, 'q> Validator<'a, 'q> {
    fn error(&mut self, message: impl Into<String>, locations: Vec<Pos>, code: &str) -> &mut Error {
        self.error_at(message, locations, self.path.clone(), code)
    }

    fn error_at(
        &mut self,
        message: impl Into<String>,
        locations: Vec<Pos>,
        path: Vec<Value>,
        code: &str,
    ) -> &mut Error {
        let error = Error::new(message)
            .with_locations(locations)
            .with_path(path)
            .with_extension("code", code);
        self.errors.push(error);
        self.errors.last_mut().expect("an error was pushed")
    }

    fn document(&mut self) {
        let document = self.document;
        for (index, definition) in document.definitions.iter().enumerate() {
            self.current = index;
            match definition {
                Definition::Operation(operation) => self.operation(operation),
                Definition::Fragment(fragment) => self.fragment(fragment),
                Definition::TypeSystem(pos) => self.schema_definitions.push(*pos),
            }
        }
        self.path.clear();
        // The rules that report at the end, in the reverse order of graphql-ruby's list.
        self.fragment_variable_usages();
        self.variables_used_and_defined();
        self.named_spreads_possible();
        let (unmet, unused, cyclical) = self.dependencies();
        for (name, pos, path) in unmet {
            self.error_at(
                format!("Fragment {name} was used, but not defined"),
                vec![pos],
                path,
                "useAndDefineFragment",
            )
            .extend("fragmentName", name);
        }
        for fragment in unused {
            self.error_at(
                format!("Fragment {} was defined, but not used", fragment.name),
                vec![fragment.pos],
                vec![format!("fragment {}", fragment.name).into()],
                "useAndDefineFragment",
            )
            .extend("fragmentName", fragment.name);
        }
        for fragment in cyclical {
            self.error_at(
                format!("Fragment {} contains an infinite loop", fragment.name),
                vec![fragment.pos],
                vec![format!("fragment {}", fragment.name).into()],
                "infiniteLoop",
            )
            .extend("fragmentName", fragment.name);
        }
        for (name, places) in std::mem::take(&mut self.fragment_names) {
            if places.len() > 1 {
                self.error(
                    format!("Fragment name \"{name}\" must be unique"),
                    places,
                    "fragmentNotUnique",
                )
                .extend("fragmentName", name);
            }
        }
        let names = std::mem::take(&mut self.operation_names);
        let count: usize = names.values().map(Vec::len).sum();
        for (name, places) in names {
            match name {
                None if count > 1 => {
                    self.error(
                        "Operation name is required when multiple operations are present",
                        places,
                        "uniquelyNamedOperations",
                    );
                }
                Some(name) if places.len() > 1 => {
                    self.error(
                        format!("Operation name \"{name}\" must be unique"),
                        places,
                        "uniquelyNamedOperations",
                    )
                    .extend("operationName", name);
                }
                _ => {}
            }
        }
        if !self.schema_definitions.is_empty() {
            let places = std::mem::take(&mut self.schema_definitions);
            self.error(
                "Query cannot contain schema definitions",
                places,
                "queryContainsSchemaDefinitions",
            );
        }
    }

    // ---- definitions ----

    fn operation(&mut self, operation: &'a Operation<'q>) {
        self.path = vec![label(operation.kind, operation.name).into()];
        let schema = self.schema;
        let root = match operation.kind {
            OperationKind::Query => Some(schema.query_type()),
            OperationKind::Mutation => schema.mutation_type(),
            OperationKind::Subscription => schema.subscription_type(),
        }
        .and_then(|name| schema.get(name));
        self.unique_directives(&operation.directives);
        self.operation_names
            .entry(operation.name)
            .or_default()
            .push(operation.pos);
        if let Some(root) = root {
            self.check_merge(&operation.selection_set, root);
        }
        // Variables are unique.
        let mut by_name: IndexMap<&str, Vec<Pos>> = IndexMap::new();
        for variable in &operation.variables {
            by_name.entry(variable.name).or_default().push(variable.pos);
        }
        for (name, places) in by_name {
            if places.len() > 1 {
                self.error(
                    format!("There can only be one variable named \"{name}\""),
                    places,
                    "variableNotUnique",
                )
                .extend("variableName", name);
            }
        }
        let mut usage = Usage::default();
        for variable in &operation.variables {
            let entry = usage.variables.entry(variable.name).or_default();
            entry.declared = true;
            entry.declared_path = self.path.clone();
        }
        self.usages.insert(self.current, usage);
        self.in_fragment = false;
        self.declared = operation.variables.iter().map(|v| (v.name, v)).collect();
        if root.is_none() {
            let (message, code) = match operation.kind {
                OperationKind::Mutation => (
                    "Schema is not configured for mutations",
                    "missingMutationConfiguration",
                ),
                OperationKind::Subscription => (
                    "Schema is not configured for subscriptions",
                    "missingSubscriptionConfiguration",
                ),
                OperationKind::Query => (
                    "Schema is not configured for queries",
                    "missingQueryConfiguration",
                ),
            };
            self.error(message, vec![operation.pos], code);
            return;
        }
        for variable in &operation.variables {
            self.variable_definition(variable);
        }
        let location = match operation.kind {
            OperationKind::Query => DirectiveLocation::Query,
            OperationKind::Mutation => DirectiveLocation::Mutation,
            OperationKind::Subscription => DirectiveLocation::Subscription,
        };
        for directive in &operation.directives {
            self.directive(directive, location);
        }
        self.selections(&operation.selection_set, root, operation.pos);
    }

    fn variable_definition(&mut self, variable: &'a VariableDefinition<'q>) {
        let type_name = variable.ty.named();
        match self.schema.get(type_name) {
            None => {
                self.error(
                    format!(
                        "{type_name} isn't a defined input type (on ${})",
                        variable.name
                    ),
                    vec![variable.pos],
                    "variableRequiresValidType",
                )
                .extend("typeName", type_name)
                .extend("variableName", variable.name);
            }
            Some(def) if !def.is_input() => {
                self.error(
                    format!(
                        "{type_name} isn't a valid input type (on ${})",
                        variable.name
                    ),
                    vec![variable.pos],
                    "variableRequiresValidType",
                )
                .extend("typeName", type_name)
                .extend("variableName", variable.name);
            }
            Some(_) => {}
        }
        if let Some(default) = &variable.default_value
            && self.schema.get(type_name).is_some()
            && let Err(problem) = input::validate_literal(self.schema, default, Some(&variable.ty))
        {
            let message = problem
                .map(|problem| problem.explanation)
                .unwrap_or_else(|| {
                    format!(
                        "Default value for ${} doesn't match type {}",
                        variable.name, variable.ty
                    )
                });
            self.error(message, vec![variable.pos], "defaultValueInvalidType")
                .extend("variableName", variable.name)
                .extend("typeName", variable.ty.to_string());
        }
        for directive in &variable.directives {
            self.directive(directive, DirectiveLocation::VariableDefinition);
        }
    }

    fn fragment(&mut self, fragment: &'a Fragment<'q>) {
        self.path = vec![format!("fragment {}", fragment.name).into()];
        self.unique_directives(&fragment.directives);
        self.fragment_names
            .entry(fragment.name)
            .or_default()
            .push(fragment.pos);
        let Some(ty) = self.fragment_type(Some(fragment.type_condition), None, fragment.pos) else {
            return;
        };
        self.usages.insert(self.current, Usage::default());
        self.in_fragment = true;
        for directive in &fragment.directives {
            self.directive(directive, DirectiveLocation::FragmentDefinition);
        }
        self.selections(&fragment.selection_set, Some(ty), fragment.pos);
    }

    /// The type of a fragment's condition: an error when it does not exist or is not an
    /// object, an interface or a union (`FragmentTypesExist`, `FragmentsAreOnCompositeTypes`).
    fn fragment_type(
        &mut self,
        condition: Option<&str>,
        parent: Option<&'a TypeDef>,
        pos: Pos,
    ) -> Option<&'a TypeDef> {
        let Some(name) = condition else {
            return parent;
        };
        let Some(ty) = self.schema.get(name) else {
            self.error(
                format!("No such type {name}, so it can't be a fragment condition"),
                vec![pos],
                "undefinedType",
            )
            .extend("typeName", name);
            return None;
        };
        if !ty.is_composite() {
            self.error(
                format!("Invalid fragment on type {name} (must be Union, Interface or Object)"),
                vec![pos],
                "fragmentOnNonCompositeType",
            )
            .extend("typeName", name);
            return None;
        }
        Some(ty)
    }

    // ---- selections ----

    fn selections(&mut self, set: &'a SelectionSet<'q>, parent: Option<&'a TypeDef>, at: Pos) {
        for selection in &set.items {
            match selection {
                Selection::Field(field) => self.field(field, parent, at),
                Selection::InlineFragment(inline) => self.inline_fragment(inline, parent),
                Selection::FragmentSpread(spread) => self.fragment_spread(spread, parent),
            }
        }
    }

    fn field(&mut self, field: &'a Field<'q>, parent: Option<&'a TypeDef>, parent_pos: Pos) {
        self.path.push(field.response_key().into());
        self.field_inner(field, parent, parent_pos);
        self.path.pop();
    }

    fn field_inner(&mut self, field: &'a Field<'q>, parent: Option<&'a TypeDef>, parent_pos: Pos) {
        let schema = self.schema;
        self.unique_directives(&field.directives);
        let def = parent.and_then(|parent| schema.lookup_field(&parent.name, field.name));
        if let (Some(parent), None) = (parent, def) {
            if parent.kind == Kind::Union {
                self.error(
                    format!(
                        "Selections can't be made directly on unions (see selections on {})",
                        parent.name
                    ),
                    vec![parent_pos],
                    "selectionMismatch",
                )
                .extend("nodeName", parent.name.as_str());
            } else {
                self.error(
                    format!(
                        "Field '{}' doesn't exist on type '{}'",
                        field.name, parent.name
                    ),
                    vec![field.pos],
                    "undefinedField",
                )
                .extend("typeName", parent.name.as_str())
                .extend("fieldName", field.name);
            }
            return;
        }
        let ty = def.and_then(|def| schema.get(def.ty.named()));
        if let Some(ty) = ty
            && !field.selection_set.is_empty()
            && may_conflict(&field.selection_set)
        {
            self.check_merge(&field.selection_set, ty);
        }
        if !self.appropriate_selections(field, ty) {
            return;
        }
        if let Some(def) = def {
            self.required_arguments("Field", field.name, field.pos, &field.arguments, &def.args);
        }
        self.unique_arguments(&field.arguments);
        let owner = Owner::Field(field.response_key(), def, field.pos);
        for argument in &field.arguments {
            self.argument(argument, owner);
        }
        for directive in &field.directives {
            self.directive(directive, DirectiveLocation::Field);
        }
        self.selections(&field.selection_set, ty, field.pos);
    }

    /// `FieldsHaveAppropriateSelections`: a leaf has no selection, the others have one.
    fn appropriate_selections(&mut self, field: &'a Field<'q>, ty: Option<&'a TypeDef>) -> bool {
        let Some(ty) = ty else {
            return true;
        };
        let node_name = format!("field '{}'", field.name);
        if ty.is_leaf() && !field.selection_set.is_empty() {
            let selections: Vec<String> = field
                .selection_set
                .items
                .iter()
                .map(|selection| match selection {
                    Selection::Field(field) => format!("\"{}\"", field.name),
                    Selection::FragmentSpread(spread) => format!("\"{}\"", spread.name),
                    Selection::InlineFragment(inline) => match inline.type_condition {
                        Some(name) => format!("\"... on {name} {{ ... }}\""),
                        None => "\"... { ... }\"".to_string(),
                    },
                })
                .collect();
            let kind = if ty.kind == Kind::Enum {
                "enums"
            } else {
                "scalars"
            };
            self.error(
                format!(
                    "Selections can't be made on {kind} ({node_name} returns {} but has selections [{}])",
                    ty.name,
                    selections.join(", ")
                ),
                vec![field.pos],
                "selectionMismatch",
            )
            .extend("nodeName", node_name)
            .extend("typeName", ty.name.as_str());
            return false;
        }
        if !ty.is_leaf() && field.selection_set.is_empty() {
            self.error(
                format!(
                    "Field must have selections ({node_name} returns {} but has no selections. Did you mean '{} {{ ... }}'?)",
                    ty.name, field.name
                ),
                vec![field.pos],
                "selectionMismatch",
            )
            .extend("nodeName", node_name)
            .extend("typeName", ty.name.as_str());
            return false;
        }
        true
    }

    fn inline_fragment(&mut self, inline: &'a InlineFragment<'q>, parent: Option<&'a TypeDef>) {
        let label = match inline.type_condition {
            Some(name) => format!("... on {name}"),
            None => "...".to_string(),
        };
        self.path.push(label.into());
        self.unique_directives(&inline.directives);
        if let Some(ty) = self.fragment_type(inline.type_condition, parent, inline.pos) {
            if let Some(parent) = parent {
                self.spread_possible(None, parent, ty, inline.pos, self.path.clone());
            }
            for directive in &inline.directives {
                self.directive(directive, DirectiveLocation::InlineFragment);
            }
            self.selections(&inline.selection_set, Some(ty), inline.pos);
        }
        self.path.pop();
    }

    fn fragment_spread(&mut self, spread: &'a FragmentSpread<'q>, parent: Option<&'a TypeDef>) {
        self.path.push(format!("... {}", spread.name).into());
        self.unique_directives(&spread.directives);
        self.spreads.push((spread, parent, self.path.clone()));
        let path = self.path.clone();
        if let Some(usage) = self.usages.get_mut(&self.current) {
            usage.spreads.push((spread.name, spread.pos, path));
        }
        for directive in &spread.directives {
            self.directive(directive, DirectiveLocation::FragmentSpread);
        }
        self.path.pop();
    }

    /// `FragmentSpreadsArePossible`: some object can be both the type spread into and the
    /// type of the fragment. Unions are not checked, as in graphql-ruby.
    fn spread_possible(
        &mut self,
        name: Option<&str>,
        parent: &TypeDef,
        child: &TypeDef,
        pos: Pos,
        path: Vec<Value>,
    ) {
        if !matches!(child.kind, Kind::Object | Kind::Interface) {
            return;
        }
        let parents = self.schema.possible_types(&parent.name);
        let overlap = self
            .schema
            .possible_types(&child.name)
            .iter()
            .any(|object| parents.contains(object));
        if !overlap {
            let label = name.map(|name| format!(" {name}")).unwrap_or_default();
            self.error_at(
                format!(
                    "Fragment{label} on {} can't be spread inside {}",
                    child.name, parent.name
                ),
                vec![pos],
                path,
                "cannotSpreadFragment",
            )
            .extend("typeName", child.name.as_str())
            .extend(
                "fragmentName",
                if label.is_empty() {
                    "unknown".to_string()
                } else {
                    label
                },
            )
            .extend("parentName", parent.name.as_str());
        }
    }

    // ---- arguments and values ----

    /// `RequiredArgumentsArePresent`.
    fn required_arguments(
        &mut self,
        class: &str,
        name: &str,
        pos: Pos,
        given: &[Argument<'q>],
        defined: &IndexMap<String, InputValue>,
    ) {
        let missing: Vec<&str> = defined
            .values()
            .filter(|arg| arg.is_required() && !given.iter().any(|g| g.name == arg.name))
            .map(|arg| arg.name.as_str())
            .collect();
        if !missing.is_empty() {
            let missing = missing.join(", ");
            self.error(
                format!("{class} '{name}' is missing required arguments: {missing}"),
                vec![pos],
                "missingRequiredArguments",
            )
            .extend("className", class)
            .extend("name", name)
            .extend("arguments", missing);
        }
    }

    /// `ArgumentNamesAreUnique`.
    fn unique_arguments(&mut self, arguments: &[Argument<'q>]) {
        let mut by_name: IndexMap<&str, Vec<Pos>> = IndexMap::new();
        for argument in arguments {
            by_name.entry(argument.name).or_default().push(argument.pos);
        }
        for (name, places) in by_name {
            if places.len() > 1 {
                self.error(
                    format!("There can be only one argument named \"{name}\""),
                    places,
                    "argumentNotUnique",
                )
                .extend("name", name);
            }
        }
    }

    fn argument(&mut self, argument: &'a Argument<'q>, owner: Owner<'a>) {
        self.path.push(argument.name.into());
        let def = owner.argument(argument.name);
        if owner.is_known() && def.is_none() {
            // `ArgumentsAreDefined`, which stops there.
            self.error(
                format!(
                    "{} '{}' doesn't accept argument '{}'",
                    owner.kind(),
                    owner.name(),
                    argument.name
                ),
                vec![argument.pos],
                "argumentNotAccepted",
            )
            .extend("name", owner.name())
            .extend("typeName", owner.kind())
            .extend("argumentName", argument.name);
            self.path.pop();
            return;
        }
        if let Some(def) = def {
            self.variable_usages(argument, def);
        }
        self.value(&argument.value, def, None);
        // `ArgumentLiteralsAreCompatible`, once what is inside is checked, and only when nothing
        // inside was refused.
        if argument.value.variable().is_none()
            && let Some(def) = def
            && !self.has_error_under_path()
            && let Err(problem) =
                input::validate_literal(self.schema, &argument.value, Some(&def.ty))
        {
            let printed = print::literal(&argument.value);
            let coercion = problem.and_then(|problem| problem.message);
            let message = coercion.clone().unwrap_or_else(|| {
                format!(
                    "Argument '{}' on {} '{}' has an invalid value ({printed}). Expected type '{}'.",
                    argument.name,
                    owner.kind(),
                    owner.name(),
                    def.ty
                )
            });
            let error = self.error(message, vec![owner.pos()], "argumentLiteralsIncompatible");
            if coercion.is_some() {
                error.extend("typeName", "CoercionError");
            } else {
                error
                    .extend("typeName", owner.kind())
                    .extend("argumentName", argument.name);
            }
        }
        self.path.pop();
    }

    fn has_error_under_path(&self) -> bool {
        self.errors.iter().any(|error| {
            error
                .path
                .as_ref()
                .is_some_and(|path| path.starts_with(&self.path))
        })
    }

    /// Visits what is inside a value: the variables it uses, and the input objects in it.
    fn value(&mut self, value: &'a Value_<'q>, def: Option<&'a InputValue>, index: Option<usize>) {
        match &value.kind {
            ValueKind::Variable(name) => {
                let used = (value.pos, self.path.clone(), self.current);
                if let Some(usage) = self.usages.get_mut(&self.current) {
                    usage.variables.entry(name).or_default().used = Some(used);
                }
            }
            ValueKind::Object(fields) => self.input_object(value, fields, def, index),
            ValueKind::List(items) => {
                for (index, item) in items.iter().enumerate() {
                    self.value(item, def, Some(index));
                }
            }
            _ => {}
        }
    }

    fn input_object(
        &mut self,
        value: &'a Value_<'q>,
        fields: &'a [Argument<'q>],
        def: Option<&'a InputValue>,
        index: Option<usize>,
    ) {
        // An object in a list is at its index in the path.
        let in_list = index.is_some() && def.is_some_and(|def| def.ty.is_list());
        if let (true, Some(index)) = (in_list, index) {
            self.path.push(index.into());
        }
        let schema = self.schema;
        let ty = def
            .and_then(|def| schema.get(def.ty.named()))
            .filter(|ty| ty.kind == Kind::InputObject);
        if let Some(ty) = ty {
            for field in ty.input_fields.values() {
                if field.is_required() && !fields.iter().any(|f| f.name == field.name) {
                    let mut path = self.path.clone();
                    path.push(field.name.clone().into());
                    self.error_at(
                        format!(
                            "Argument '{}' on InputObject '{}' is required. Expected type {}",
                            field.name, ty.name, field.ty
                        ),
                        vec![value.pos],
                        path,
                        "missingRequiredInputObjectAttribute",
                    )
                    .extend("argumentName", field.name.as_str())
                    .extend("argumentType", field.ty.to_string())
                    .extend("inputObjectType", ty.name.as_str());
                }
            }
        }
        let mut by_name: IndexMap<&str, Vec<Pos>> = IndexMap::new();
        for field in fields {
            by_name.entry(field.name).or_default().push(field.pos);
        }
        for (name, places) in by_name {
            if places.len() > 1 {
                self.error(
                    format!("There can be only one input field named \"{name}\""),
                    places,
                    "inputFieldNotUnique",
                )
                .extend("name", name);
            }
        }
        if let Some(ty) = ty.filter(|ty| ty.one_of) {
            self.one_of(value, fields, ty);
        }
        for field in fields {
            self.argument(field, Owner::InputObject(ty, value.pos));
        }
        if in_list {
            self.path.pop();
        }
    }

    /// `OneOfInputObjectsAreValid`.
    fn one_of(&mut self, value: &'a Value_<'q>, fields: &'a [Argument<'q>], ty: &TypeDef) {
        let [field] = fields else {
            self.error(
                format!(
                    "OneOf Input Object '{}' must specify exactly one key.",
                    ty.name
                ),
                vec![value.pos],
                "invalidOneOfInputObject",
            )
            .extend("inputObjectType", ty.name.as_str());
            return;
        };
        let mut path = self.path.clone();
        path.push(field.name.into());
        if field.value.is_null() {
            self.error_at(
                format!("Argument '{}.{}' must be non-null.", ty.name, field.name),
                vec![field.pos],
                path,
                "invalidOneOfInputObject",
            )
            .extend("inputObjectType", ty.name.as_str());
        } else if let Some(variable) = field.value.variable()
            && let Some(declared) = self.declared.get(variable)
            && !declared.ty.is_non_null()
        {
            self.error_at(
                format!(
                    "Variable '{variable}' must be non-nullable to be used for OneOf Input Object '{}'.",
                    ty.name
                ),
                vec![value.pos],
                path,
                "invalidOneOfInputObject",
            )
            .extend("inputObjectType", ty.name.as_str());
        }
    }

    // ---- directives ----

    fn directive(&mut self, directive: &'a Directive<'q>, location: DirectiveLocation) {
        let Some(def) = self.schema.directive(directive.name) else {
            match self.undefined_directives.get(directive.name) {
                Some(&index) => {
                    if let Some(locations) = &mut self.errors[index].locations {
                        locations.push(directive.pos);
                    }
                }
                None => {
                    self.undefined_directives
                        .insert(directive.name, self.errors.len());
                    self.error(
                        format!("Directive @{} is not defined", directive.name),
                        vec![directive.pos],
                        "undefinedDirective",
                    )
                    .extend("directiveName", directive.name);
                }
            }
            return;
        };
        if !def.locations.contains(&location) {
            let target = location_name(location).unwrap_or_default();
            let allowed: Vec<&str> = def
                .locations
                .iter()
                .map(|l| location_name(*l).unwrap_or_default())
                .collect();
            self.error(
                format!(
                    "'@{}' can't be applied to {target} (allowed: {})",
                    def.name,
                    allowed.join(", ")
                ),
                vec![directive.pos],
                "directiveCannotBeApplied",
            )
            .extend("targetName", target)
            .extend("name", def.name.as_str());
        }
        self.required_arguments(
            "Directive",
            directive.name,
            directive.pos,
            &directive.arguments,
            &def.args,
        );
        self.unique_arguments(&directive.arguments);
        let owner = Owner::Directive(directive.name, Some(def), directive.pos);
        for argument in &directive.arguments {
            self.argument(argument, owner);
        }
    }

    /// `UniqueDirectivesPerLocation`: a directive that is not repeatable is used once.
    fn unique_directives(&mut self, directives: &[Directive<'q>]) {
        if directives.len() < 2 {
            return;
        }
        let mut first: IndexMap<&str, Vec<Pos>> = IndexMap::new();
        for directive in directives {
            let repeatable = self
                .schema
                .directive(directive.name)
                .is_some_and(|def| def.repeatable);
            if !repeatable {
                first.entry(directive.name).or_default().push(directive.pos);
            }
        }
        for (name, places) in first {
            if places.len() > 1 {
                self.error(
                    format!("The directive \"{name}\" can only be used once at this location."),
                    places,
                    "directiveNotUniqueForLocation",
                )
                .extend("directiveName", name);
            }
        }
    }

    // ---- variables ----

    /// `VariableUsagesAreAllowed`, for the variables an argument is given (directly, or in a
    /// list). In a fragment, they are checked at the end, against each operation.
    fn variable_usages(&mut self, argument: &'a Argument<'q>, def: &'a InputValue) {
        let mut variables = Vec::new();
        variables_in(&argument.value, 0, &mut variables);
        for (variable, list_depth) in variables {
            if self.in_fragment {
                let usage = ArgumentUse {
                    argument,
                    definition: def,
                    variable,
                    list_depth,
                    path: self.path.clone(),
                };
                if let Some(usages) = self.usages.get_mut(&self.current) {
                    usages.arguments.push(usage);
                }
            } else if let Some(declared) = self.declared.get(variable).copied() {
                let path = self.path.clone();
                self.check_usage(argument, def, declared, list_depth, path);
            }
        }
    }

    /// Whether a variable may be given where an argument is (the spec's "IsVariableUsageAllowed",
    /// with graphql-ruby's messages).
    fn check_usage(
        &mut self,
        argument: &Argument<'q>,
        def: &InputValue,
        declared: &VariableDefinition<'q>,
        list_depth: usize,
        path: Vec<Value>,
    ) -> bool {
        let mut variable_type = declared.ty.clone();
        let has_default = declared
            .default_value
            .as_ref()
            .is_some_and(|default| !default.is_null());
        if has_default && !variable_type.is_non_null() {
            variable_type = TypeRef::NonNull(Box::new(variable_type));
        }
        for _ in 0..list_depth {
            variable_type = TypeRef::NonNull(Box::new(TypeRef::List(Box::new(variable_type))));
        }
        let mut argument_type = &def.ty;
        if def.default_value.is_some() {
            argument_type = argument_type.nullable();
        }
        let problem = if variable_type.named() != argument_type.named() {
            "Type mismatch"
        } else if variable_type.list_depth() != argument_type.list_depth() {
            "List dimension mismatch"
        } else if !non_null_levels_match(argument_type, &variable_type) {
            "Nullability mismatch"
        } else {
            return false;
        };
        let message = format!(
            "{problem} on variable ${} and argument {} ({variable_type} / {})",
            declared.name, argument.name, def.ty
        );
        self.error_at(message, vec![argument.pos], path, "variableMismatch")
            .extend("variableName", declared.name)
            .extend("typeName", variable_type.to_string())
            .extend("argumentName", argument.name)
            .extend("errorMessage", problem);
        true
    }

    /// The usages of variables in fragments, against each operation that spreads them.
    fn fragment_variable_usages(&mut self) {
        let document = self.document;
        let mut reported: HashSet<(Pos, String)> = HashSet::new();
        for (index, definition) in document.definitions.iter().enumerate() {
            let Definition::Operation(operation) = definition else {
                continue;
            };
            if operation.variables.is_empty() {
                continue;
            }
            let declared: HashMap<&str, &VariableDefinition<'_>> =
                operation.variables.iter().map(|v| (v.name, v)).collect();
            for fragment in self.fragments_used_by(index) {
                let usages: Vec<_> = self
                    .usages
                    .get(&fragment)
                    .map(|usage| {
                        usage
                            .arguments
                            .iter()
                            .map(|u| {
                                (
                                    u.argument,
                                    u.definition,
                                    u.variable,
                                    u.list_depth,
                                    u.path.clone(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                for (argument, def, variable, list_depth, path) in usages {
                    let Some(declared) = declared.get(variable) else {
                        continue;
                    };
                    let count = self.errors.len();
                    if self.check_usage(argument, def, declared, list_depth, path) {
                        let message = self.errors[count].message.clone();
                        if !reported.insert((argument.pos, message)) {
                            self.errors.truncate(count);
                        }
                    }
                }
            }
        }
    }

    /// The fragments a definition spreads, directly or not, by their index in the document.
    fn fragments_used_by(&self, definition: usize) -> Vec<usize> {
        let mut seen = Vec::new();
        let mut stack = vec![definition];
        while let Some(index) = stack.pop() {
            let Some(usage) = self.usages.get(&index) else {
                continue;
            };
            for (name, ..) in &usage.spreads {
                if let Some(fragment) = self.fragment_index(name)
                    && !seen.contains(&fragment)
                {
                    seen.push(fragment);
                    stack.push(fragment);
                }
            }
        }
        seen
    }

    fn fragment_index(&self, name: &str) -> Option<usize> {
        self.document
            .definitions
            .iter()
            .position(|d| matches!(d, Definition::Fragment(f) if f.name == name))
    }

    /// `VariablesAreUsedAndDefined`: every variable an operation declares is used by it or by
    /// the fragments it spreads, and every variable they use is declared.
    fn variables_used_and_defined(&mut self) {
        let document = self.document;
        let operations: Vec<usize> = self
            .usages
            .keys()
            .copied()
            .filter(|index| matches!(document.definitions[*index], Definition::Operation(_)))
            .collect();
        for index in operations {
            let mut variables = self.usages[&index].variables.clone();
            let mut visited = Vec::new();
            self.follow_spreads(index, &mut variables, &mut visited, 0);
            let Definition::Operation(operation) = &document.definitions[index] else {
                continue;
            };
            let name_of = |definition: usize| match &document.definitions[definition] {
                Definition::Operation(op) => op
                    .name
                    .map_or_else(|| format!("anonymous {}", op.kind.as_str()), str::to_string),
                Definition::Fragment(fragment) => fragment.name.to_string(),
                Definition::TypeSystem(_) => String::new(),
            };
            for (name, usage) in &variables {
                if usage.declared && usage.used.is_none() {
                    self.error_at(
                        format!(
                            "Variable ${name} is declared by {} but not used",
                            name_of(index)
                        ),
                        vec![operation.pos],
                        usage.declared_path.clone(),
                        "variableNotUsed",
                    )
                    .extend("variableName", *name);
                }
            }
            for (name, usage) in &variables {
                if let (false, Some((pos, path, by))) = (usage.declared, &usage.used) {
                    self.error_at(
                        format!(
                            "Variable ${name} is used by {} but not declared",
                            name_of(*by)
                        ),
                        vec![*pos],
                        path.clone(),
                        "variableNotDefined",
                    )
                    .extend("variableName", *name);
                }
            }
        }
    }

    fn follow_spreads(
        &self,
        definition: usize,
        variables: &mut IndexMap<&'q str, VariableUse>,
        visited: &mut Vec<&'q str>,
        depth: usize,
    ) {
        let Some(usage) = self.usages.get(&definition).filter(|_| depth <= MAX_DEPTH) else {
            return;
        };
        let spreads: Vec<&'q str> = usage
            .spreads
            .iter()
            .map(|(name, ..)| *name)
            .filter(|name| !visited.contains(name))
            .collect();
        for name in spreads {
            let Some(fragment) = self
                .fragment_index(name)
                .filter(|f| self.usages.contains_key(f))
            else {
                continue;
            };
            visited.push(name);
            for (variable, child) in &self.usages[&fragment].variables {
                let parent = variables.entry(variable).or_default();
                if child.used.is_some() {
                    parent.used.clone_from(&child.used);
                }
            }
            self.follow_spreads(fragment, variables, visited, depth + 1);
        }
    }

    // ---- fragments ----

    fn named_spreads_possible(&mut self) {
        for (spread, parent, path) in std::mem::take(&mut self.spreads) {
            let Some(parent) = parent else {
                continue;
            };
            let Some(fragment) = self.document.fragment(spread.name) else {
                continue;
            };
            if let Some(child) = self.schema.get(fragment.type_condition) {
                self.spread_possible(Some(spread.name), parent, child, spread.pos, path);
            }
        }
    }

    /// graphql-ruby's resolution of the fragments each definition depends on: the spreads of
    /// fragments that do not exist (unmet), the fragments no definition spreads (unused), and the
    /// fragments that never resolve because they spread themselves, directly or not (cyclical).
    #[allow(clippy::type_complexity)]
    fn dependencies(
        &self,
    ) -> (
        Vec<(&'q str, Pos, Vec<Value>)>,
        Vec<&'a Fragment<'q>>,
        Vec<&'a Fragment<'q>>,
    ) {
        let document = self.document;
        // The fragments by name, in order, then the fragments flattened in that order.
        let mut by_name: IndexMap<&str, Vec<usize>> = IndexMap::new();
        for (index, definition) in document.definitions.iter().enumerate() {
            if let Definition::Fragment(fragment) = definition {
                by_name.entry(fragment.name).or_default().push(index);
            }
        }
        let fragments: Vec<usize> = by_name.values().flatten().copied().collect();
        // What each definition spreads (that still waits to resolve), and who spreads a name.
        let mut immediate: IndexMap<usize, Vec<(&'q str, Pos, Vec<Value>)>> = IndexMap::new();
        let mut dependents: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, usage) in &self.usages {
            for spread in &usage.spreads {
                immediate.entry(*index).or_default().push(spread.clone());
                let users = dependents.entry(spread.0).or_default();
                if !users.contains(index) {
                    users.push(*index);
                }
            }
        }
        let mut independent: Vec<usize> = fragments
            .iter()
            .copied()
            .filter(|index| !immediate.contains_key(index))
            .collect();
        let mut visited: HashSet<&str> = HashSet::new();
        let mut unused = Vec::new();
        let fragment = |index: usize| match &document.definitions[index] {
            Definition::Fragment(fragment) => Some(fragment),
            _ => None,
        };
        while let Some(index) = independent.pop() {
            let Some(resolved) = fragment(index) else {
                continue;
            };
            if !visited.insert(resolved.name) {
                continue;
            }
            immediate.shift_remove(&index);
            let users = dependents.get(resolved.name).cloned().unwrap_or_default();
            if users.is_empty() {
                unused.push(resolved);
                continue;
            }
            for user in users {
                let remaining = immediate.entry(user).or_default();
                remaining.retain(|(name, ..)| *name != resolved.name);
                if remaining.is_empty()
                    && let Some(user_fragment) = fragment(user)
                    && user_fragment.name != resolved.name
                {
                    independent.push(user);
                }
            }
        }
        let mut unmet = Vec::new();
        immediate.retain(|_, spreads| {
            spreads.retain(|spread| {
                let defined = by_name.contains_key(spread.0);
                if !defined {
                    unmet.push(spread.clone());
                }
                defined
            });
            !spreads.is_empty()
        });
        let cyclical = immediate
            .keys()
            .filter_map(|index| fragment(*index))
            .collect();
        (unmet, unused, cyclical)
    }

    // ---- fields that can be merged ----

    /// `FieldsWillMerge` for a selection set: two fields with the same response key ask for the
    /// same field with the same arguments (unless they are on two different object types), and
    /// return values of the same shape.
    fn check_merge(&mut self, set: &'a SelectionSet<'q>, parent: &'a TypeDef) {
        let mut fields = IndexMap::new();
        self.collect_fields(set, parent, &mut fields, &mut Vec::new(), 0);
        let mut conflicts = Conflicts::default();
        for (key, group) in &fields {
            for (i, first) in group.iter().enumerate() {
                for second in &group[i + 1..] {
                    self.find_conflict(key, first, second, false, &mut conflicts, 0);
                }
            }
        }
        for (_, by_key) in conflicts.0 {
            for (key, (places, alternatives, kind)) in by_key {
                let article = if kind == "argument" { "an" } else { "a" };
                let alternatives = alternatives.join(" or ");
                self.error_at(
                    format!("Field '{key}' has {article} {kind} conflict: {alternatives}?"),
                    places,
                    Vec::new(),
                    "fieldConflict",
                )
                .extend("fieldName", key)
                .extend("conflicts", alternatives);
            }
        }
    }

    fn collect_fields(
        &self,
        set: &'a SelectionSet<'q>,
        parent: &'a TypeDef,
        out: &mut IndexMap<&'q str, Vec<MergeField<'a, 'q>>>,
        visited: &mut Vec<&'q str>,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let mut spreads = Vec::new();
        for selection in &set.items {
            match selection {
                Selection::Field(field) => {
                    out.entry(field.response_key())
                        .or_default()
                        .push(MergeField {
                            node: field,
                            def: self.schema.lookup_field(&parent.name, field.name),
                            parent,
                        });
                }
                Selection::InlineFragment(inline) => {
                    let ty = match inline.type_condition {
                        Some(name) => self.schema.get(name),
                        None => Some(parent),
                    };
                    if let Some(ty) = ty {
                        self.collect_fields(&inline.selection_set, ty, out, visited, depth + 1);
                    }
                }
                Selection::FragmentSpread(spread) => spreads.push(spread.name),
            }
        }
        for name in spreads {
            if visited.contains(&name) {
                continue;
            }
            visited.push(name);
            let Some(fragment) = self.document.fragment(name) else {
                continue;
            };
            if let Some(ty) = self.schema.get(fragment.type_condition) {
                self.collect_fields(&fragment.selection_set, ty, out, visited, depth + 1);
            }
        }
    }

    fn find_conflict(
        &mut self,
        key: &'q str,
        first: &MergeField<'a, 'q>,
        second: &MergeField<'a, 'q>,
        parents_exclusive: bool,
        conflicts: &mut Conflicts<'q>,
        depth: usize,
    ) {
        let (Some(first_def), Some(second_def)) = (first.def, second.def) else {
            return;
        };
        if depth > MAX_DEPTH {
            return;
        }
        if std::ptr::eq(first.node, second.node) {
            return;
        }
        let exclusive = parents_exclusive
            || (first.parent.name != second.parent.name
                && first.parent.kind == Kind::Object
                && second.parent.kind == Kind::Object);
        if !exclusive {
            let different_names = first.node.name != second.node.name;
            let different_arguments = !same_arguments(first.node, second.node);
            if different_names {
                let pair = ("field", first.node.pos, second.node.pos);
                if self.reported_conflicts.insert(pair) {
                    conflicts.add("field", key, first.node.pos, first.node.name.to_string());
                    conflicts.add("field", key, second.node.pos, second.node.name.to_string());
                }
            }
            if different_arguments {
                let pair = ("argument", first.node.pos, second.node.pos);
                if self.reported_conflicts.insert(pair) {
                    conflicts.add("argument", key, first.node.pos, merge_arguments(first.node));
                    conflicts.add(
                        "argument",
                        key,
                        second.node.pos,
                        merge_arguments(second.node),
                    );
                }
            }
            if different_names || different_arguments {
                return;
            }
        }
        if !conflicts.has("field", key)
            && !std::ptr::eq(first_def, second_def)
            && return_types_conflict(self.schema, &first_def.ty, &second_def.ty)
        {
            let pair = ("return_type", first.node.pos, second.node.pos);
            if self.reported_conflicts.insert(pair) {
                conflicts.add(
                    "return_type",
                    key,
                    first.node.pos,
                    format!("`{}`", first_def.ty),
                );
                conflicts.add(
                    "return_type",
                    key,
                    second.node.pos,
                    format!("`{}`", second_def.ty),
                );
            }
        }
        if first.node.selection_set.is_empty() && second.node.selection_set.is_empty() {
            return;
        }
        let (Some(first_type), Some(second_type)) = (
            self.schema.get(first_def.ty.named()),
            self.schema.get(second_def.ty.named()),
        ) else {
            return;
        };
        let mut first_fields = IndexMap::new();
        self.collect_fields(
            &first.node.selection_set,
            first_type,
            &mut first_fields,
            &mut Vec::new(),
            0,
        );
        let mut second_fields = IndexMap::new();
        self.collect_fields(
            &second.node.selection_set,
            second_type,
            &mut second_fields,
            &mut Vec::new(),
            0,
        );
        for (sub_key, firsts) in &first_fields {
            let Some(seconds) = second_fields.get(sub_key) else {
                continue;
            };
            for a in firsts {
                for b in seconds {
                    self.find_conflict(sub_key, a, b, exclusive, conflicts, depth + 1);
                }
            }
        }
    }
}

type Value_<'q> = super::ast::Value<'q>;

/// A field of a selection set, with the type it is selected on.
struct MergeField<'a, 'q> {
    node: &'a Field<'q>,
    def: Option<&'a FieldDef>,
    parent: &'a TypeDef,
}

/// The conflicts found in a selection set, by kind (`field`, `argument`, `return_type`) then
/// response key: where they are, and what conflicts.
#[derive(Default)]
#[allow(clippy::type_complexity)]
struct Conflicts<'q>(
    IndexMap<&'static str, IndexMap<&'q str, (Vec<Pos>, Vec<String>, &'static str)>>,
);

impl<'q> Conflicts<'q> {
    fn add(&mut self, kind: &'static str, key: &'q str, pos: Pos, alternative: String) {
        let entry = self
            .0
            .entry(kind)
            .or_default()
            .entry(key)
            .or_insert_with(|| (Vec::new(), Vec::new(), kind));
        if !entry.0.contains(&pos) {
            entry.0.push(pos);
            entry.1.push(alternative);
        }
    }

    fn has(&self, kind: &str, key: &str) -> bool {
        self.0
            .get(kind)
            .is_some_and(|by_key| by_key.contains_key(key))
    }
}

/// graphql-ruby's `selections_may_conflict?`: whether a selection set needs the merge check.
fn may_conflict(set: &SelectionSet<'_>) -> bool {
    let mut names = Vec::new();
    for selection in &set.items {
        match selection {
            Selection::Field(field) if field.alias.is_none() => names.push(field.name),
            _ => return true,
        }
    }
    if names.len() > 8 {
        return true;
    }
    names
        .iter()
        .enumerate()
        .any(|(i, name)| names[i + 1..].contains(name))
}

fn same_arguments(first: &Field<'_>, second: &Field<'_>) -> bool {
    first.arguments.len() == second.arguments.len()
        && first.arguments.iter().all(|a| {
            second
                .argument(a.name)
                .is_some_and(|b| print::literal(&a.value) == print::literal(&b.value))
        })
}

/// The arguments of a field as graphql-ruby writes them in an argument conflict:
/// `{first:"1",query:"\"a\""}`.
fn merge_arguments(field: &Field<'_>) -> String {
    let arguments: Vec<String> = field
        .arguments
        .iter()
        .map(|a| format!("{}:{}", a.name, print::string(&print::literal(&a.value))))
        .collect();
    format!("{{{}}}", arguments.join(","))
}

/// Whether two types cannot be merged in one response: one is a list or non-null and not the
/// other, or two different leaf types.
fn return_types_conflict(schema: &Schema, first: &TypeRef, second: &TypeRef) -> bool {
    match (first, second) {
        (TypeRef::NonNull(a), TypeRef::NonNull(b)) | (TypeRef::List(a), TypeRef::List(b)) => {
            return_types_conflict(schema, a, b)
        }
        (TypeRef::NonNull(_), _) | (_, TypeRef::NonNull(_)) => true,
        (TypeRef::List(_), _) | (_, TypeRef::List(_)) => true,
        (TypeRef::Named(a), TypeRef::Named(b)) => {
            let leaf = |name: &str| schema.get(name).is_some_and(TypeDef::is_leaf);
            (leaf(a) || leaf(b)) && a != b
        }
    }
}

/// The variables a value is (or, in a list, has), with how many lists they are in.
fn variables_in<'q>(value: &Value_<'q>, depth: usize, out: &mut Vec<(&'q str, usize)>) {
    match &value.kind {
        ValueKind::Variable(name) => out.push((name, depth)),
        ValueKind::List(items) => {
            for item in items {
                variables_in(item, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// graphql-ruby's `non_null_levels_match`.
fn non_null_levels_match(argument: &TypeRef, variable: &TypeRef) -> bool {
    if argument.is_non_null() && !variable.is_non_null() {
        return false;
    }
    let inner = |ty: &TypeRef| match ty {
        TypeRef::NonNull(inner) | TypeRef::List(inner) => Some(inner.as_ref().clone()),
        TypeRef::Named(_) => None,
    };
    match (inner(argument), inner(variable)) {
        (Some(argument_inner), Some(_)) => {
            let variable = if variable.is_non_null() && !argument.is_non_null() {
                variable.nullable().clone()
            } else {
                variable.clone()
            };
            match inner(&variable) {
                Some(variable_inner) => non_null_levels_match(&argument_inner, &variable_inner),
                None => true,
            }
        }
        _ => true,
    }
}

/// How graphql-ruby names a location in its messages; the locations of the type system have no
/// name there.
fn location_name(location: DirectiveLocation) -> Option<&'static str> {
    Some(match location {
        DirectiveLocation::Query => "queries",
        DirectiveLocation::Mutation => "mutations",
        DirectiveLocation::Subscription => "subscriptions",
        DirectiveLocation::Field => "fields",
        DirectiveLocation::FragmentDefinition => "fragment definitions",
        DirectiveLocation::FragmentSpread => "fragment spreads",
        DirectiveLocation::InlineFragment => "inline fragments",
        DirectiveLocation::VariableDefinition => "variable definitions",
        _ => return None,
    })
}

trait Extensions {
    fn extend(&mut self, key: &str, value: impl Into<Value>) -> &mut Self;
}

impl Extensions for Error {
    fn extend(&mut self, key: &str, value: impl Into<Value>) -> &mut Error {
        self.extensions
            .get_or_insert_with(Default::default)
            .insert(key.to_string(), value.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::admin::schema::admin;

    fn errors(query: &str) -> Vec<Value> {
        let document = super::super::parse::parse(query).unwrap();
        validate(admin(), &document)
            .iter()
            .map(Error::to_json)
            .collect()
    }

    /// The one error of a query.
    fn error(query: &str) -> Value {
        let errors = errors(query);
        assert_eq!(errors.len(), 1, "{query}: {errors:#?}");
        errors.into_iter().next().unwrap()
    }

    fn messages(query: &str) -> Vec<String> {
        errors(query)
            .iter()
            .map(|e| e["message"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn a_valid_query_has_no_error() {
        let query = r#"
            query Products($first: Int = 2, $query: String, $withTags: Boolean!) {
              shop { name currencyCode }
              products(first: $first, query: $query, sortKey: TITLE) {
                edges { cursor node { ...ProductFields tags @include(if: $withTags) } }
                pageInfo { hasNextPage endCursor }
              }
              node(id: "gid://shopify/Product/1") { __typename id ... on Product { title } }
              productByIdentifier(identifier: {handle: "shirt"}) { id }
            }
            fragment ProductFields on Product { id title status }
        "#;
        assert_eq!(errors(query), Vec::<Value>::new());
        let mutation = r#"
            mutation Set($metafields: [MetafieldsSetInput!]!) {
              metafieldsSet(metafields: $metafields) { metafields { id } userErrors { field message code } }
            }
        "#;
        assert_eq!(errors(mutation), Vec::<Value>::new());
    }

    #[test]
    fn fields_are_defined_on_their_type() {
        assert_eq!(
            error("{ shop { foo } }"),
            json!({
                "message": "Field 'foo' doesn't exist on type 'Shop'",
                "locations": [{"line": 1, "column": 10}],
                "path": ["query", "shop", "foo"],
                "extensions": {"code": "undefinedField", "typeName": "Shop", "fieldName": "foo"}
            })
        );
        // An unknown field is not checked further.
        assert_eq!(errors("{ shop { foo(x: 1) { bar } } }").len(), 1);
        assert_eq!(
            error("{ shop { metafield(key: \"a\") { reference { id } } } }"),
            json!({
                "message": "Selections can't be made directly on unions (see selections on MetafieldReference)",
                "locations": [{"line": 1, "column": 32}],
                "path": ["query", "shop", "metafield", "reference", "id"],
                "extensions": {"code": "selectionMismatch", "nodeName": "MetafieldReference"}
            })
        );
    }

    #[test]
    fn leaves_have_no_selection_and_objects_have_one() {
        assert_eq!(
            error("{ shop { name { x } } }"),
            json!({
                "message": "Selections can't be made on scalars (field 'name' returns String but has selections [\"x\"])",
                "locations": [{"line": 1, "column": 10}],
                "path": ["query", "shop", "name"],
                "extensions": {"code": "selectionMismatch", "nodeName": "field 'name'", "typeName": "String"}
            })
        );
        assert_eq!(
            error("query { shop }")["message"],
            "Field must have selections (field 'shop' returns Shop but has no selections. Did you mean 'shop { ... }'?)"
        );
        assert_eq!(
            messages(
                "{ product(id: \"gid://shopify/Product/1\") { status { a ... on X { b } } } }"
            ),
            [
                "Selections can't be made on enums (field 'status' returns ProductStatus but has selections [\"a\", \"... on X { ... }\"])"
            ]
        );
    }

    #[test]
    fn arguments_are_known_unique_and_required() {
        assert_eq!(
            error("{ shop(x: 1) { name } }"),
            json!({
                "message": "Field 'shop' doesn't accept argument 'x'",
                "locations": [{"line": 1, "column": 8}],
                "path": ["query", "shop", "x"],
                "extensions": {"code": "argumentNotAccepted", "name": "shop", "typeName": "Field", "argumentName": "x"}
            })
        );
        assert_eq!(
            error("{ s: shop @skip(if: false, unless: true) { name } }")["message"],
            "Directive 'skip' doesn't accept argument 'unless'"
        );
        assert_eq!(
            error("{ product { id } }"),
            json!({
                "message": "Field 'product' is missing required arguments: id",
                "locations": [{"line": 1, "column": 3}],
                "path": ["query", "product"],
                "extensions": {"code": "missingRequiredArguments", "className": "Field", "name": "product", "arguments": "id"}
            })
        );
        assert_eq!(
            error("{ products(first: 1, first: 2) { nodes { id } } }"),
            json!({
                "message": "There can be only one argument named \"first\"",
                "locations": [{"line": 1, "column": 12}, {"line": 1, "column": 22}],
                "path": ["query", "products"],
                "extensions": {"code": "argumentNotUnique", "name": "first"}
            })
        );
    }

    #[test]
    fn literals_fit_their_type() {
        assert_eq!(
            error("{ products(first: \"x\") { nodes { id } } }"),
            json!({
                "message": "Argument 'first' on Field 'products' has an invalid value (\"x\"). Expected type 'Int'.",
                "locations": [{"line": 1, "column": 3}],
                "path": ["query", "products", "first"],
                "extensions": {"code": "argumentLiteralsIncompatible", "typeName": "Field", "argumentName": "first"}
            })
        );
        // A coercion error of a scalar is its own message.
        assert_eq!(
            error("{ product(id: \"1\") { id } }"),
            json!({
                "message": "Invalid global id `1`",
                "locations": [{"line": 1, "column": 3}],
                "path": ["query", "product", "id"],
                "extensions": {"code": "argumentLiteralsIncompatible", "typeName": "CoercionError"}
            })
        );
        assert_eq!(
            error("{ products(first: 1, sortKey: NAME) { nodes { id } } }")["message"],
            "Argument 'sortKey' on Field 'products' has an invalid value (NAME). Expected type 'ProductSortKeys'."
        );
        assert_eq!(
            error("{ nodes(ids: [\"gid://shopify/Product/1\", null]) { id } }")["message"],
            "Argument 'ids' on Field 'nodes' has an invalid value ([\"gid://shopify/Product/1\", null]). Expected type '[ID!]!'."
        );
        // An unknown field of an input object is reported, not the whole value.
        assert_eq!(
            error("{ productByIdentifier(identifier: {sku: \"a\"}) { id } }"),
            json!({
                "message": "InputObject 'ProductIdentifierInput' doesn't accept argument 'sku'",
                "locations": [{"line": 1, "column": 36}],
                "path": ["query", "productByIdentifier", "identifier", "sku"],
                "extensions": {"code": "argumentNotAccepted", "name": "ProductIdentifierInput", "typeName": "InputObject", "argumentName": "sku"}
            })
        );
    }

    #[test]
    fn input_objects_have_their_required_fields_once_each() {
        let query = "mutation { metafieldsSet(metafields: [{key: \"a\", value: \"b\"}]) { userErrors { message } } }";
        assert_eq!(
            error(query),
            json!({
                "message": "Argument 'ownerId' on InputObject 'MetafieldsSetInput' is required. Expected type ID!",
                "locations": [{"line": 1, "column": 39}],
                "path": ["mutation", "metafieldsSet", "metafields", 0, "ownerId"],
                "extensions": {
                    "code": "missingRequiredInputObjectAttribute",
                    "argumentName": "ownerId",
                    "argumentType": "ID!",
                    "inputObjectType": "MetafieldsSetInput"
                }
            })
        );
        assert_eq!(
            messages("{ productByIdentifier(identifier: {handle: \"a\", handle: \"b\"}) { id } }"),
            [
                "There can be only one input field named \"handle\"",
                "OneOf Input Object 'ProductIdentifierInput' must specify exactly one key.",
            ]
        );
    }

    #[test]
    fn one_of_input_objects_have_one_field() {
        assert_eq!(
            error(
                "{ productByIdentifier(identifier: {id: \"gid://shopify/Product/1\", handle: \"a\"}) { id } }"
            ),
            json!({
                "message": "OneOf Input Object 'ProductIdentifierInput' must specify exactly one key.",
                "locations": [{"line": 1, "column": 35}],
                "path": ["query", "productByIdentifier", "identifier"],
                "extensions": {"code": "invalidOneOfInputObject", "inputObjectType": "ProductIdentifierInput"}
            })
        );
        assert_eq!(
            error("{ productByIdentifier(identifier: {handle: null}) { id } }")["message"],
            "Argument 'ProductIdentifierInput.handle' must be non-null."
        );
        assert_eq!(
            error("query Q($h: String) { productByIdentifier(identifier: {handle: $h}) { id } }")["message"],
            "Variable 'h' must be non-nullable to be used for OneOf Input Object 'ProductIdentifierInput'."
        );
    }

    #[test]
    fn fragments_are_known_used_possible_and_finite() {
        assert_eq!(
            error("{ shop { ... on Foo { id } } }"),
            json!({
                "message": "No such type Foo, so it can't be a fragment condition",
                "locations": [{"line": 1, "column": 10}],
                "path": ["query", "shop", "... on Foo"],
                "extensions": {"code": "undefinedType", "typeName": "Foo"}
            })
        );
        assert_eq!(
            error("{ shop { ... on String { id } } }")["message"],
            "Invalid fragment on type String (must be Union, Interface or Object)"
        );
        assert_eq!(
            error("{ shop { ... on Product { id } } }"),
            json!({
                "message": "Fragment on Product can't be spread inside Shop",
                "locations": [{"line": 1, "column": 10}],
                "path": ["query", "shop", "... on Product"],
                "extensions": {"code": "cannotSpreadFragment", "typeName": "Product", "fragmentName": "unknown", "parentName": "Shop"}
            })
        );
        assert_eq!(
            error("{ shop { ...P } } fragment P on Product { id }"),
            json!({
                "message": "Fragment P on Product can't be spread inside Shop",
                "locations": [{"line": 1, "column": 10}],
                "path": ["query", "shop", "... P"],
                "extensions": {"code": "cannotSpreadFragment", "typeName": "Product", "fragmentName": " P", "parentName": "Shop"}
            })
        );
        assert_eq!(
            error("{ shop { name } } fragment F on Shop { name }"),
            json!({
                "message": "Fragment F was defined, but not used",
                "locations": [{"line": 1, "column": 19}],
                "path": ["fragment F"],
                "extensions": {"code": "useAndDefineFragment", "fragmentName": "F"}
            })
        );
        assert_eq!(
            error("{ shop { ...F } }"),
            json!({
                "message": "Fragment F was used, but not defined",
                "locations": [{"line": 1, "column": 10}],
                "path": ["query", "shop", "... F"],
                "extensions": {"code": "useAndDefineFragment", "fragmentName": "F"}
            })
        );
        assert_eq!(
            messages("{ shop { ...A } } fragment A on Shop { ...B } fragment B on Shop { ...A }"),
            [
                "Fragment A contains an infinite loop",
                "Fragment B contains an infinite loop",
            ]
        );
        assert_eq!(
            error("{ shop { ...F } } fragment F on Shop { name } fragment F on Shop { id }"),
            json!({
                "message": "Fragment name \"F\" must be unique",
                "locations": [{"line": 1, "column": 19}, {"line": 1, "column": 47}],
                "path": [],
                "extensions": {"code": "fragmentNotUnique", "fragmentName": "F"}
            })
        );
    }

    #[test]
    fn operations_are_named_uniquely() {
        assert_eq!(
            error("query A { shop { name } } query A { shop { id } }"),
            json!({
                "message": "Operation name \"A\" must be unique",
                "locations": [{"line": 1, "column": 1}, {"line": 1, "column": 27}],
                "path": [],
                "extensions": {"code": "uniquelyNamedOperations", "operationName": "A"}
            })
        );
        assert_eq!(
            error("{ shop { name } } query B { shop { id } }"),
            json!({
                "message": "Operation name is required when multiple operations are present",
                "locations": [{"line": 1, "column": 1}],
                "path": [],
                "extensions": {"code": "uniquelyNamedOperations"}
            })
        );
        assert_eq!(
            error("subscription { shop { name } }"),
            json!({
                "message": "Schema is not configured for subscriptions",
                "locations": [{"line": 1, "column": 1}],
                "path": ["subscription"],
                "extensions": {"code": "missingSubscriptionConfiguration"}
            })
        );
        assert_eq!(
            error("type Foo { a: Int }\n{ shop { name } }"),
            json!({
                "message": "Query cannot contain schema definitions",
                "locations": [{"line": 1, "column": 1}],
                "path": [],
                "extensions": {"code": "queryContainsSchemaDefinitions"}
            })
        );
    }

    #[test]
    fn variables_are_defined_used_unique_and_of_input_types() {
        assert_eq!(
            error("query Q($id: ID!) { shop { name } }"),
            json!({
                "message": "Variable $id is declared by Q but not used",
                "locations": [{"line": 1, "column": 1}],
                "path": ["query Q"],
                "extensions": {"code": "variableNotUsed", "variableName": "id"}
            })
        );
        assert_eq!(
            error("query { product(id: $id) { id } }"),
            json!({
                "message": "Variable $id is used by anonymous query but not declared",
                "locations": [{"line": 1, "column": 21}],
                "path": ["query", "product", "id"],
                "extensions": {"code": "variableNotDefined", "variableName": "id"}
            })
        );
        assert_eq!(
            error("query Q { shop { ...F } } fragment F on Shop { metafield(key: $key) { id } }")["message"],
            "Variable $key is used by F but not declared"
        );
        assert!(errors("query Q($key: String!) { shop { ...F } } fragment F on Shop { metafield(key: $key) { id } }").is_empty());
        assert_eq!(
            error("query Q($a: Int, $a: Int) { products(first: $a) { nodes { id } } }"),
            json!({
                "message": "There can only be one variable named \"a\"",
                "locations": [{"line": 1, "column": 9}, {"line": 1, "column": 18}],
                "path": ["query Q"],
                "extensions": {"code": "variableNotUnique", "variableName": "a"}
            })
        );
        assert_eq!(
            messages("query Q($a: Shop, $b: Foo) { shop { name } }"),
            [
                "Shop isn't a valid input type (on $a)",
                "Foo isn't a defined input type (on $b)",
                "Variable $a is declared by Q but not used",
                "Variable $b is declared by Q but not used",
            ]
        );
        assert_eq!(
            errors("query Q($a: Shop) { shop { name } }")[0]["extensions"],
            json!({"code": "variableRequiresValidType", "typeName": "Shop", "variableName": "a"})
        );
        assert_eq!(
            error("query Q($n: Int = \"x\") { products(first: $n) { nodes { id } } }"),
            json!({
                "message": "Could not coerce value \"x\" to Int",
                "locations": [{"line": 1, "column": 9}],
                "path": ["query Q"],
                "extensions": {"code": "defaultValueInvalidType", "variableName": "n", "typeName": "Int"}
            })
        );
    }

    #[test]
    fn variables_are_used_where_their_type_is_allowed() {
        assert_eq!(
            error("query Q($id: String!) { product(id: $id) { id } }"),
            json!({
                "message": "Type mismatch on variable $id and argument id (String! / ID!)",
                "locations": [{"line": 1, "column": 33}],
                "path": ["query Q", "product", "id"],
                "extensions": {
                    "code": "variableMismatch",
                    "variableName": "id",
                    "typeName": "String!",
                    "argumentName": "id",
                    "errorMessage": "Type mismatch"
                }
            })
        );
        assert_eq!(
            error("query Q($id: ID) { product(id: $id) { id } }")["message"],
            "Nullability mismatch on variable $id and argument id (ID / ID!)"
        );
        assert_eq!(
            error("query Q($ids: ID!) { nodes(ids: $ids) { id } }")["message"],
            "List dimension mismatch on variable $ids and argument ids (ID! / [ID!]!)"
        );
        // In a list, with a default, or for an argument with a default: allowed.
        assert!(errors("query Q($id: ID!) { nodes(ids: [$id]) { id } }").is_empty());
        assert!(errors("query Q($n: Int = 5) { products(first: $n) { nodes { id } } }").is_empty());
        assert!(
            errors("query Q($r: Boolean) { products(first: 1, reverse: $r) { nodes { id } } }")
                .is_empty()
        );
        assert_eq!(
            error(
                "query Q($id: ID) { shop { ...F } } fragment F on Shop { m: metafield(key: \"a\") { reference { ... on Product { id } } } p: metafield(key: $id) { id } }"
            )["message"],
            "Type mismatch on variable $id and argument key (ID / String!)"
        );
    }

    #[test]
    fn directives_are_known_in_their_place_and_unique() {
        assert_eq!(
            error("{ shop @foo { name } a: shop @foo { id } }"),
            json!({
                "message": "Directive @foo is not defined",
                "locations": [{"line": 1, "column": 8}, {"line": 1, "column": 30}],
                "path": ["query", "shop"],
                "extensions": {"code": "undefinedDirective", "directiveName": "foo"}
            })
        );
        assert_eq!(
            error("query Q @skip(if: true) { shop { name } }"),
            json!({
                "message": "'@skip' can't be applied to queries (allowed: fields, fragment spreads, inline fragments)",
                "locations": [{"line": 1, "column": 9}],
                "path": ["query Q"],
                "extensions": {"code": "directiveCannotBeApplied", "targetName": "queries", "name": "skip"}
            })
        );
        assert_eq!(
            error("{ shop @skip(if: false) @skip(if: false) { name } }"),
            json!({
                "message": "The directive \"skip\" can only be used once at this location.",
                "locations": [{"line": 1, "column": 8}, {"line": 1, "column": 25}],
                "path": ["query", "shop"],
                "extensions": {"code": "directiveNotUniqueForLocation", "directiveName": "skip"}
            })
        );
        assert_eq!(
            error("{ shop @skip { name } }")["message"],
            "Directive 'skip' is missing required arguments: if"
        );
        assert!(errors("mutation { productCreate(product: {title: \"a\"}) @idempotent(key: \"k\") { product { id } } }").is_empty());
    }

    #[test]
    fn a_long_chain_of_fragments_does_not_run_out_of_stack() {
        let mut query = String::from("{ shop { ...F0 } }\n");
        for i in 0..2000 {
            query.push_str(&format!("fragment F{i} on Shop {{ name ...F{} }}\n", i + 1));
        }
        query.push_str("fragment F2000 on Shop { id }");
        assert!(errors(&query).is_empty());
    }

    #[test]
    fn fields_with_one_response_key_can_be_merged() {
        assert_eq!(
            error("{ shop { name: id name } }"),
            json!({
                "message": "Field 'name' has a field conflict: id or name?",
                "locations": [{"line": 1, "column": 10}, {"line": 1, "column": 19}],
                "path": [],
                "extensions": {"code": "fieldConflict", "fieldName": "name", "conflicts": "id or name"}
            })
        );
        assert_eq!(
            error("{ products(first: 1) { nodes { id } } products(first: 2) { nodes { id } } }"),
            json!({
                "message": "Field 'products' has an argument conflict: {first:\"1\"} or {first:\"2\"}?",
                "locations": [{"line": 1, "column": 3}, {"line": 1, "column": 39}],
                "path": [],
                "extensions": {"code": "fieldConflict", "fieldName": "products", "conflicts": "{first:\"1\"} or {first:\"2\"}"}
            })
        );
        assert_eq!(
            error(
                "{ node(id: \"gid://shopify/Product/1\") { ... on Product { x: title } ... on Collection { x: updatedAt } } }"
            )["message"],
            "Field 'x' has a return_type conflict: `String!` or `DateTime!`?"
        );
        // On two object types, two fields of a key need not be the same field.
        assert!(errors("{ node(id: \"gid://shopify/Product/1\") { ... on Product { x: title } ... on Collection { x: handle } } }").is_empty());
        // Deeper, through the selections of two fields of a key.
        assert_eq!(
            error("{ shop { a: name } shop { a: email } }")["message"],
            "Field 'a' has a field conflict: name or email?"
        );
        assert!(errors("{ shop { name } shop { name id } }").is_empty());
    }
}
