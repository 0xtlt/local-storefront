//! The `typescript-operations` plugin of graphql-codegen: one `<Name>Variables` type and one
//! result type for each operation, one type for each fragment, with the types of the selected
//! fields resolved (no `Pick` of the schema types).
//!
//! The code follows `SelectionSetToObject` of graphql-codegen's `visitor-plugin-common` step by
//! step, quirks included: the order the fields come out in, which fields an inline fragment adds
//! to which type, how a field selected twice is merged. The tests compare the output with the
//! real thing.

use std::collections::{BTreeSet, HashMap};

use graphql_parser::Pos;
use graphql_parser::query as q;
use indexmap::IndexMap;

use super::model::{Kind, Model, TypeRef};
use super::text::{convert_name, strip_trailing_spaces};
use super::types::{VariableType, variable_line};
use super::{Options, Scalar};

pub(super) type Selection = q::Selection<'static, String>;
pub(super) type Field = q::Field<'static, String>;
pub(super) type SelectionSet = q::SelectionSet<'static, String>;
pub(super) type Directive = q::Directive<'static, String>;
pub(super) type Definition = q::Definition<'static, String>;

/// A definition of a document, found in a file.
pub(super) struct Source {
    pub path: String,
    pub definition: Definition,
}

/// What a node that is collected for an object type can be: a field of the selection, the fields
/// a fragment spread brings, or a directive of the inline fragment they came from.
#[derive(Clone)]
enum Node {
    Field(Field),
    Spread(SpreadUsage),
    Directive(Directive),
}

/// A fragment spread, as it applies to one type.
#[derive(Clone)]
struct SpreadUsage {
    on_type: String,
    selections: Vec<Selection>,
}

/// An inline fragment, or the fields of a selection set wrapped as one.
struct Inline {
    type_condition: Option<String>,
    directives: Vec<Directive>,
    selections: Vec<Selection>,
}

type TypeMap = HashMap<String, Vec<Node>>;

/// The fields selected on each possible type of a type (in the order of the possible types), and
/// whether a possible type has none.
type Grouped = (Vec<(String, Option<String>)>, bool);

pub(super) struct Operations<'a> {
    pub model: &'a Model,
    pub options: &'a Options,
    pub scalars: &'a IndexMap<String, Scalar>,
    /// Every fragment of the documents, in order.
    pub fragments: Vec<(String, q::FragmentDefinition<'static, String>)>,
    /// The types already written for a selection set (by where it is in its file) and the fields
    /// and possible types it was selected with; see `transform_selection_set`.
    cache: HashMap<Option<(usize, usize)>, HashMap<String, String>>,
    unnamed: usize,
}

impl<'a> Operations<'a> {
    pub fn new(
        model: &'a Model,
        options: &'a Options,
        scalars: &'a IndexMap<String, Scalar>,
        sources: &[Source],
    ) -> Operations<'a> {
        let fragments = sources
            .iter()
            .filter_map(|source| match &source.definition {
                q::Definition::Fragment(fragment) => {
                    Some((fragment.name.clone(), fragment.clone()))
                }
                q::Definition::Operation(_) => None,
            })
            .collect();
        Operations {
            model,
            options,
            scalars,
            fragments,
            cache: HashMap::new(),
            unnamed: 0,
        }
    }

    /// The content of the plugin: the types of every definition, in the order of the documents.
    pub fn content(&mut self, sources: &[Source]) -> Result<String, String> {
        let mut definitions = Vec::new();
        for source in sources {
            let text = match &source.definition {
                q::Definition::Operation(operation) => self.operation(operation),
                q::Definition::Fragment(fragment) => self.fragment(fragment),
            };
            definitions.push(text.map_err(|error| format!("{}: {error}", source.path))?);
        }
        Ok(definitions.join("\n"))
    }

    fn convert(&self, name: &str) -> String {
        convert_name(name, false)
    }

    /// The suffix of the type of an operation or a fragment: `Query`, `Fragment`, ...
    fn suffix(&self, name: &str, kind: &str) -> String {
        if self.options.omit_operation_suffix
            || (self.options.dedupe_operation_suffix
                && name.to_lowercase().ends_with(&kind.to_lowercase()))
        {
            String::new()
        } else {
            kind.to_string()
        }
    }

    fn operation(
        &mut self,
        operation: &q::OperationDefinition<'static, String>,
    ) -> Result<String, String> {
        let (kind, name, variables, set) = match operation {
            q::OperationDefinition::SelectionSet(set) => ("query", None, &[][..], set),
            q::OperationDefinition::Query(query) => (
                "query",
                query.name.clone(),
                &query.variable_definitions[..],
                &query.selection_set,
            ),
            q::OperationDefinition::Mutation(mutation) => (
                "mutation",
                mutation.name.clone(),
                &mutation.variable_definitions[..],
                &mutation.selection_set,
            ),
            q::OperationDefinition::Subscription(subscription) => (
                "subscription",
                subscription.name.clone(),
                &subscription.variable_definitions[..],
                &subscription.selection_set,
            ),
        };
        let name = match name {
            Some(name) => self.convert(&name),
            None => {
                self.unnamed += 1;
                convert_name(&format!("Unnamed_{}_", self.unnamed), false)
            }
        };
        let root = self
            .model
            .root(kind)
            .ok_or_else(|| format!("the schema has no root type for `{kind}` operations"))?
            .to_string();
        let operation_type = pascal_first(kind);
        let suffix = self.suffix(&name, &operation_type);
        let result = self.transform_selection_set(&root, set)?;
        let lines: Vec<String> = variables
            .iter()
            .map(|variable| {
                let line = variable_line(
                    &VariableType {
                        name: &variable.name,
                        ty: &TypeRef::from_ast(&variable.var_type),
                        has_default: variable.default_value.is_some(),
                    },
                    self.scalars,
                    true,
                    self.options,
                    false,
                );
                format!("  {line}")
            })
            .collect();
        let block = if lines.is_empty() {
            "{ [key: string]: never; }".to_string()
        } else {
            format!("{{\n{};\n}}", lines.join(";\n"))
        };
        let variables_name = self.convert(&format!("{name}{suffix}Variables"));
        let result_name = self.convert(&format!(
            "{name}{suffix}{}",
            self.options.operation_result_suffix
        ));
        let variables_type = format!("export type {variables_name} = Exact<{block}>;\n");
        let result_type = format!("export type {result_name} = {result};\n");
        Ok(strip_trailing_spaces(&format!(
            "{variables_type}\n\n{result_type}"
        )))
    }

    fn fragment(
        &mut self,
        fragment: &q::FragmentDefinition<'static, String>,
    ) -> Result<String, String> {
        let q::TypeCondition::On(on_type) = &fragment.type_condition;
        let suffix = self.suffix(&fragment.name, "Fragment");
        let merged = self.fragment_type_name(&fragment.name, &suffix, "");
        let (grouped, _) = self.build_grouped_selections(on_type, &fragment.selection_set)?;
        let mut sub_types = Vec::new();
        for (type_name, set) in &grouped {
            let declaration = self.fragment_type_name(&fragment.name, &suffix, type_name);
            match set {
                None if !self.options.add_typename() => {
                    sub_types.push((declaration, "{}".to_string()));
                }
                None => {}
                Some(set) => sub_types.push((declaration, set.clone())),
            }
        }
        let declare = |export: bool, name: &str, content: &str| {
            // A fragment on an interface that nothing implements has no type to write.
            let content = if content.is_empty() { "{}" } else { content };
            strip_trailing_spaces(&format!(
                "{}type {name} = {content};\n",
                if export { "export " } else { "" }
            ))
        };
        if sub_types.len() == 1 {
            return Ok(declare(true, &merged, &sub_types[0].1));
        }
        let mut parts: Vec<String> = sub_types
            .iter()
            .map(|(name, content)| declare(false, name, content))
            .collect();
        let names: Vec<&str> = sub_types.iter().map(|(name, _)| name.as_str()).collect();
        parts.push(declare(true, &merged, &names.join(" | ")));
        Ok(parts.join("\n"))
    }

    fn fragment_type_name(&self, name: &str, suffix: &str, type_name: &str) -> String {
        let suffix = if !type_name.is_empty() && !suffix.is_empty() {
            format!("_{type_name}_{suffix}")
        } else if !type_name.is_empty() {
            format!("_{type_name}")
        } else {
            suffix.to_string()
        };
        self.convert(&format!("{name}{suffix}"))
    }

    // ---- selection sets ----

    /// The type of the data a selection set reads from `parent`, as TypeScript.
    ///
    /// graphql-codegen remembers the type it wrote for a selection set (by where the set is in
    /// its file; the sets it builds itself, by merging, are all the same place) together with the
    /// field names and possible types it was selected with, and writes that type again for the
    /// next set with the same key. That is a cache that can answer for a set that is not the same,
    /// and the output depends on it, so it is kept.
    fn transform_selection_set(
        &mut self,
        parent: &str,
        set: &SelectionSet,
    ) -> Result<String, String> {
        let mut possible: Vec<&str> = self.model.possible_types(parent);
        possible.sort_unstable();
        let mut names = BTreeSet::new();
        field_names(&set.items, "", &self.fragments, &mut names);
        let key = format!(
            "{} @ {}",
            names.into_iter().collect::<Vec<_>>().join(","),
            possible.join("|")
        );
        let location = location(set);
        if let Some(cached) = self.cache.get(&location).and_then(|map| map.get(&key))
            && !cached.is_empty()
        {
            return Ok(cached.clone());
        }
        let (grouped, must_add_empty_object) = self.build_grouped_selections(parent, set)?;
        let result = if grouped.is_empty() {
            "never".to_string()
        } else {
            let mut parts: Vec<String> = grouped.into_iter().filter_map(|(_, set)| set).collect();
            if must_add_empty_object {
                parts.push("{}".to_string());
            }
            parts.join(" | ")
        };
        self.cache
            .entry(location)
            .or_default()
            .insert(key, result.clone());
        Ok(result)
    }

    /// For each possible type of `parent`, the object type of the fields selected on it (`None`
    /// when none is), and whether a possible type has no field at all.
    fn build_grouped_selections(
        &mut self,
        parent: &str,
        set: &SelectionSet,
    ) -> Result<Grouped, String> {
        if set.items.is_empty() {
            return Ok((Vec::new(), true));
        }
        let by_type = self.flatten_selection_set(&set.items, parent)?;
        let mut must_add_empty_object = false;
        let mut grouped = Vec::new();
        for type_name in self.model.possible_types(parent) {
            let nodes = by_type.get(type_name).cloned().unwrap_or_default();
            let fields = self.build_selection_set(type_name, nodes)?;
            if fields.is_none() {
                must_add_empty_object = true;
            }
            grouped.push((type_name.to_string(), fields));
        }
        Ok((grouped, must_add_empty_object))
    }

    fn flatten_selection_set(
        &self,
        selections: &[Selection],
        parent: &str,
    ) -> Result<TypeMap, String> {
        let mut inlines = Vec::new();
        let mut fields = Vec::new();
        let mut spreads = Vec::new();
        for selection in selections {
            match selection {
                q::Selection::Field(_) => fields.push(selection.clone()),
                q::Selection::InlineFragment(inline) => inlines.push(Inline::from_ast(inline)),
                q::Selection::FragmentSpread(spread) => spreads.push(spread),
            }
        }
        if !fields.is_empty() {
            inlines.push(Inline {
                type_condition: Some(parent.to_string()),
                directives: Vec::new(),
                selections: fields,
            });
        }
        let mut types = TypeMap::new();
        self.collect_inline_fragments(parent, &inlines, &mut types)?;
        for (type_name, usages) in self.spread_usage(&spreads) {
            append(
                &mut types,
                &type_name,
                usages.into_iter().map(Node::Spread).collect(),
            );
        }
        Ok(types)
    }

    /// The fragment spreads of a selection, by the type each one applies to.
    fn spread_usage(
        &self,
        spreads: &[&q::FragmentSpread<'static, String>],
    ) -> IndexMap<String, Vec<SpreadUsage>> {
        let mut usage: IndexMap<String, Vec<SpreadUsage>> = IndexMap::new();
        for spread in spreads {
            let Some((_, fragment)) = self
                .fragments
                .iter()
                .find(|(name, _)| *name == spread.fragment_name)
            else {
                continue;
            };
            let q::TypeCondition::On(on_type) = &fragment.type_condition;
            for possible in self.model.possible_types(on_type) {
                usage
                    .entry(possible.to_string())
                    .or_default()
                    .push(SpreadUsage {
                        on_type: on_type.clone(),
                        selections: fragment.selection_set.items.clone(),
                    });
            }
        }
        usage
    }

    /// `_collectInlineFragments`: adds the fields and the fragment spreads of inline fragments to
    /// the types they apply to, recursively.
    fn collect_inline_fragments(
        &self,
        parent: &str,
        nodes: &[Inline],
        types: &mut TypeMap,
    ) -> Result<(), String> {
        let model = self.model;
        let kind = model.get(parent).map(|ty| &ty.kind);
        match kind {
            Some(Kind::Object { .. }) => {
                for node in nodes {
                    let on = node.type_condition.as_deref().unwrap_or(parent);
                    let (fields, inlines, spreads) = separate(&node.selections);
                    let usage = self.spread_usage(&spreads);
                    if is_object(model, on) {
                        append(types, on, fields.into_iter().map(Node::Field).collect());
                        append(types, on, spread_nodes(&usage, on));
                        append(
                            types,
                            on,
                            node.directives
                                .iter()
                                .cloned()
                                .map(Node::Directive)
                                .collect(),
                        );
                        self.collect_inline_fragments(on, &inlines, types)?;
                    } else if is_interface(model, on)
                        && model.interfaces(parent).iter().any(|name| name == on)
                    {
                        append(types, parent, fields.into_iter().map(Node::Field).collect());
                        append(types, parent, spread_nodes(&usage, parent));
                        self.collect_inline_fragments(on, &inlines, types)?;
                    }
                }
            }
            Some(Kind::Interface { .. }) => {
                let possible = model.possible_types(parent);
                for node in nodes {
                    let on = node.type_condition.as_deref().unwrap_or(parent);
                    let (fields, inlines, spreads) = separate(&node.selections);
                    let usage = self.spread_usage(&spreads);
                    if is_object(model, on) && possible.contains(&on) {
                        append(types, on, fields.into_iter().map(Node::Field).collect());
                        append(types, on, spread_nodes(&usage, on));
                        self.collect_inline_fragments(on, &inlines, types)?;
                    } else if is_interface(model, on) && on == parent {
                        for possible_type in &possible {
                            append(
                                types,
                                possible_type,
                                fields.iter().cloned().map(Node::Field).collect(),
                            );
                            append(types, possible_type, spread_nodes(&usage, possible_type));
                            self.collect_inline_fragments(on, &inlines, types)?;
                        }
                    } else {
                        for possible_type in &possible {
                            let Some(condition) = &node.type_condition else {
                                return Err(
                                    "an interface is spread on an interface field without a type condition"
                                        .to_string(),
                                );
                            };
                            if model.is_sub_type(possible_type, condition) {
                                append(
                                    types,
                                    possible_type,
                                    fields.iter().cloned().map(Node::Field).collect(),
                                );
                                append(types, possible_type, spread_nodes(&usage, possible_type));
                            }
                        }
                    }
                }
            }
            Some(Kind::Union { .. }) => {
                let possible = model.possible_types(parent);
                for node in nodes {
                    let on = node.type_condition.as_deref().unwrap_or(parent);
                    let (fields, inlines, spreads) = separate(&node.selections);
                    let usage = self.spread_usage(&spreads);
                    if is_object(model, on) && possible.contains(&on) {
                        append(types, on, fields.into_iter().map(Node::Field).collect());
                        append(types, on, spread_nodes(&usage, on));
                        self.collect_inline_fragments(on, &inlines, types)?;
                    } else if is_interface(model, on) {
                        let of_interface = model.possible_types(on);
                        for possible_type in &possible {
                            if of_interface.contains(possible_type) {
                                append(
                                    types,
                                    possible_type,
                                    fields.iter().cloned().map(Node::Field).collect(),
                                );
                                append(types, possible_type, spread_nodes(&usage, possible_type));
                                self.collect_inline_fragments(on, &inlines, types)?;
                            }
                        }
                    } else {
                        for possible_type in &possible {
                            append(
                                types,
                                possible_type,
                                fields.iter().cloned().map(Node::Field).collect(),
                            );
                            append(types, possible_type, spread_nodes(&usage, possible_type));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// `buildSelectionSet`: the object type (`{ __typename?: 'X', id: string }`) of the fields
    /// selected on the object type `parent`; `None` when no field is.
    fn build_selection_set(
        &mut self,
        parent: &str,
        nodes: Vec<Node>,
    ) -> Result<Option<String>, String> {
        let mut primitive: IndexMap<String, Field> = IndexMap::new();
        let mut aliased: IndexMap<String, Field> = IndexMap::new();
        let mut links: IndexMap<String, (TypeRef, Field)> = IndexMap::new();
        let mut requires_typename = false;
        let mut inline_conditional = false;
        let mut nodes = nodes;
        let mut index = 0;
        while index < nodes.len() {
            let node = nodes[index].clone();
            index += 1;
            match node {
                Node::Field(field) => {
                    if !field.selection_set.items.is_empty() {
                        let Some(selected) = self.model.field(parent, &field.name) else {
                            continue;
                        };
                        let response_name =
                            field.alias.clone().unwrap_or_else(|| field.name.clone());
                        let entry = match links.get(&response_name) {
                            Some((ty, existing)) => {
                                let mut merged = existing.clone();
                                merged.selection_set = merge_selection_sets(
                                    &existing.selection_set,
                                    &field.selection_set,
                                );
                                (ty.clone(), merged)
                            }
                            None => (selected.ty.clone(), field),
                        };
                        links.insert(response_name, entry);
                    } else if let Some(alias) = field.alias.clone() {
                        aliased.insert(alias, field);
                    } else if field.name == "__typename" {
                        requires_typename = true;
                    } else {
                        primitive.insert(field.name.clone(), field);
                    }
                }
                Node::Directive(directive) => {
                    if directive.name == "skip" || directive.name == "include" {
                        inline_conditional = true;
                    }
                }
                Node::Spread(usage) => {
                    let Some(fragment_type) = self.model.get(&usage.on_type) else {
                        return Err(format!("the type `{}` does not exist", usage.on_type));
                    };
                    let applies = parent == usage.on_type
                        || self.model.interfaces(parent).contains(&usage.on_type)
                        || matches!(&fragment_type.kind, Kind::Union { members } if members.iter().any(|m| m == parent));
                    if applies {
                        let flat = self.flatten_selection_set(&usage.selections, parent)?;
                        if let Some(found) = flat.get(parent) {
                            nodes.extend(found.iter().cloned());
                        }
                        for interface in self.model.interfaces(parent) {
                            if let Some(found) = flat.get(interface) {
                                nodes.extend(found.iter().cloned());
                            }
                        }
                    }
                }
            }
        }

        let mut link_fields = Vec::new();
        for (_, (selected_type, field)) in &links {
            let real = selected_type.base().to_string();
            let merged = self.transform_selection_set(&real, &field.selection_set)?;
            let conditional = has_conditional_directives(&field.directives) || inline_conditional;
            let name = self.format_named_field(&field.name, Some(selected_type), conditional);
            let alias = field
                .alias
                .as_ref()
                .map(|alias| self.format_named_field(alias, Some(selected_type), conditional));
            link_fields.push((
                alias.unwrap_or(name),
                self.wrap_type(&merged.replace('\n', "\n  "), selected_type),
            ));
        }

        let mut entries: Vec<(String, String)> = Vec::new();
        if let Some(typename) = self.typename_field(parent, requires_typename) {
            entries.push(typename);
        }
        for field in primitive.values() {
            entries.push(self.primitive_field(parent, field)?);
        }
        for (alias, field) in &aliased {
            entries.push(self.aliased_field(parent, alias, field)?);
        }
        entries.extend(link_fields);
        if entries.is_empty() {
            return Ok(None);
        }
        let fields: Vec<String> = entries
            .into_iter()
            .map(|(name, ty)| format!("{name}: {ty}"))
            .collect();
        Ok(Some(format!("{{ {} }}", fields.join(", "))))
    }

    fn typename_field(&self, parent: &str, queried: bool) -> Option<(String, String)> {
        let options = self.options;
        let is_root = [
            &self.model.query,
            &self.model.mutation,
            &self.model.subscription,
        ]
        .into_iter()
        .flatten()
        .any(|root| root == parent);
        if is_root && options.skip_typename_for_root && !queried {
            return None;
        }
        if options.non_optional_typename || options.add_typename() || queried {
            let optional = !queried && !options.non_optional_typename;
            return Some((
                format!(
                    "{}{}",
                    self.format_named_field("__typename", None, false),
                    if optional { "?" } else { "" }
                ),
                format!("'{parent}'"),
            ));
        }
        None
    }

    fn primitive_field(&self, parent: &str, field: &Field) -> Result<(String, String), String> {
        let schema_field = self.field_of(parent, &field.name)?;
        let conditional = has_conditional_directives(&field.directives);
        let name = self.format_named_field(&field.name, Some(&schema_field.ty), conditional);
        let base = schema_field.ty.base();
        let ty = if self.is_enum(base) {
            self.convert(base)
        } else if let Some(scalar) = self.scalars.get(base) {
            scalar.output.clone()
        } else {
            base.to_string()
        };
        Ok((name, self.wrap_type(&ty, &schema_field.ty)))
    }

    fn aliased_field(
        &self,
        parent: &str,
        alias: &str,
        field: &Field,
    ) -> Result<(String, String), String> {
        if field.name == "__typename" {
            return Ok((
                self.format_named_field(alias, None, false),
                format!("'{parent}'"),
            ));
        }
        let schema_field = self.field_of(parent, &field.name)?;
        let base = schema_field.ty.base();
        let ty = if self.is_enum(base) {
            self.convert(base)
        } else {
            self.scalars
                .get(base)
                .map(|scalar| scalar.output.clone())
                .filter(|output| !output.is_empty())
                .unwrap_or_else(|| base.to_string())
        };
        let conditional = has_conditional_directives(&field.directives);
        let name = self.format_named_field(alias, Some(&schema_field.ty), conditional);
        Ok((name, self.wrap_type(&ty, &schema_field.ty)))
    }

    fn field_of(&self, parent: &str, name: &str) -> Result<&super::model::Field, String> {
        self.model
            .field(parent, name)
            .ok_or_else(|| format!("`{parent}` has no field `{name}`"))
    }

    fn is_enum(&self, name: &str) -> bool {
        matches!(
            self.model.get(name).map(|ty| &ty.kind),
            Some(Kind::Enum { .. })
        )
    }

    /// The name of a field of an object type: `name`, or `name?` when it may be missing.
    fn format_named_field(&self, name: &str, ty: Option<&TypeRef>, conditional: bool) -> String {
        let optional = conditional || ty.is_some_and(|ty| !ty.is_non_null());
        format!(
            "{}{name}{}",
            if self.options.immutable_types {
                "readonly "
            } else {
                ""
            },
            if optional { "?" } else { "" }
        )
    }

    /// `wrapTypeWithModifiers`: `T | null` for a nullable type, `Array<T>` for a list.
    fn wrap_type(&self, base: &str, ty: &TypeRef) -> String {
        let array = if self.options.immutable_types {
            "ReadonlyArray"
        } else {
            "Array"
        };
        match ty {
            TypeRef::NonNull(inner) => match &**inner {
                TypeRef::List(element) => format!("{array}<{}>", self.wrap_type(base, element)),
                _ => base.to_string(),
            },
            TypeRef::List(element) => format!("{array}<{}> | null", self.wrap_type(base, element)),
            TypeRef::Named(_) => format!("{base} | null"),
        }
    }
}

fn pascal_first(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

impl Inline {
    fn from_ast(inline: &q::InlineFragment<'static, String>) -> Inline {
        Inline {
            type_condition: inline.type_condition.as_ref().map(|condition| {
                let q::TypeCondition::On(name) = condition;
                name.clone()
            }),
            directives: inline.directives.clone(),
            selections: inline.selection_set.items.clone(),
        }
    }
}

fn separate(
    selections: &[Selection],
) -> (
    Vec<Field>,
    Vec<Inline>,
    Vec<&q::FragmentSpread<'static, String>>,
) {
    let mut fields = Vec::new();
    let mut inlines = Vec::new();
    let mut spreads = Vec::new();
    for selection in selections {
        match selection {
            q::Selection::Field(field) => fields.push(field.clone()),
            q::Selection::InlineFragment(inline) => inlines.push(Inline::from_ast(inline)),
            q::Selection::FragmentSpread(spread) => spreads.push(spread),
        }
    }
    (fields, inlines, spreads)
}

fn spread_nodes(usage: &IndexMap<String, Vec<SpreadUsage>>, type_name: &str) -> Vec<Node> {
    usage
        .get(type_name)
        .map(|usages| usages.iter().cloned().map(Node::Spread).collect())
        .unwrap_or_default()
}

fn append(types: &mut TypeMap, type_name: &str, nodes: Vec<Node>) {
    types
        .entry(type_name.to_string())
        .or_default()
        .extend(nodes);
}

fn is_object(model: &Model, name: &str) -> bool {
    matches!(
        model.get(name).map(|ty| &ty.kind),
        Some(Kind::Object { .. })
    )
}

fn is_interface(model: &Model, name: &str) -> bool {
    matches!(
        model.get(name).map(|ty| &ty.kind),
        Some(Kind::Interface { .. })
    )
}

fn has_conditional_directives(directives: &[Directive]) -> bool {
    directives
        .iter()
        .any(|directive| directive.name == "skip" || directive.name == "include")
}

/// Where a selection set is in its file; `None` for a set that graphql-codegen made by merging
/// two (it has no location).
fn location(set: &SelectionSet) -> Option<(usize, usize)> {
    let start = set.span.0;
    (start != Pos { line: 0, column: 0 }).then_some((start.line, start.column))
}

/// `getFieldNames`: the response names of every field of a selection, as paths (`a.b@c` for the
/// field `c` of `b`, aliased `b`...), with what the fragment spreads select.
fn field_names(
    selections: &[Selection],
    parent: &str,
    fragments: &[(String, q::FragmentDefinition<'static, String>)],
    names: &mut BTreeSet<String>,
) {
    for selection in selections {
        match selection {
            q::Selection::Field(field) => {
                let full = match &field.alias {
                    Some(alias) => format!("{alias}@{}", field.name),
                    None => field.name.clone(),
                };
                let full = if parent.is_empty() {
                    full
                } else {
                    format!("{parent}.{full}")
                };
                names.insert(full.clone());
                field_names(&field.selection_set.items, &full, fragments, names);
            }
            q::Selection::FragmentSpread(spread) => {
                for (name, fragment) in fragments {
                    if *name == spread.fragment_name {
                        field_names(&fragment.selection_set.items, parent, fragments, names);
                    }
                }
            }
            q::Selection::InlineFragment(inline) => {
                field_names(&inline.selection_set.items, parent, fragments, names);
            }
        }
    }
}

/// `mergeSelectionSets`: the two selection sets as one; a field of the second that the first
/// has too keeps its own place, with the selections of both.
fn merge_selection_sets(first: &SelectionSet, second: &SelectionSet) -> SelectionSet {
    let mut items = first.items.clone();
    for selection in &second.items {
        let q::Selection::Field(field) = selection else {
            items.push(selection.clone());
            continue;
        };
        let key = field.alias.as_deref().unwrap_or(&field.name);
        let found = items.iter().find_map(|item| match item {
            q::Selection::Field(other) if other.alias.as_deref().unwrap_or(&other.name) == key => {
                Some(other.clone())
            }
            _ => None,
        });
        let mut field = field.clone();
        if let Some(other) = found
            && !other.selection_set.items.is_empty()
            && !field.selection_set.items.is_empty()
        {
            field.selection_set = merge_selection_sets(&other.selection_set, &field.selection_set);
        }
        items.push(q::Selection::Field(field));
    }
    q::SelectionSet {
        span: (Pos { line: 0, column: 0 }, Pos { line: 0, column: 0 }),
        items,
    }
}
