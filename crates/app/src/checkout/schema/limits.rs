//! Shopify's input query limits (<https://shopify.dev/docs/api/functions/latest>, "Input query
//! limits"): the size of the query without its comments, the length of a list given to an
//! argument or a variable, and the calculated cost of the query. Shopify refuses a query over
//! one of them before the Function runs.

use std::collections::{BTreeMap, HashMap};

use graphql_parser::query::{self as q, Definition, Selection, SelectionSet};
use serde_json::Value;

use super::Schema;

/// "The maximum size for an input query, excluding comments, is 3000 bytes."
const MAX_SIZE: usize = 3000;
/// "Field arguments and input query variables of list type can't exceed 100 elements."
const MAX_LIST: usize = 100;
/// "Function input queries can have a maximum calculated query cost of 30."
const MAX_COST: u32 = 30;

type Fragments<'q> = HashMap<&'q str, &'q q::FragmentDefinition<'q, String>>;

impl Schema {
    /// Checks the input query `source`, parsed as `document`, and the values of its
    /// `variables` against Shopify's input query limits. The error lists every limit the
    /// query is over.
    pub fn check_limits<'q>(
        &self,
        source: &str,
        document: &'q q::Document<'q, String>,
        variables: &serde_json::Map<String, Value>,
    ) -> Result<(), String> {
        let mut problems = Vec::new();
        let size = size_without_comments(source);
        if size > MAX_SIZE {
            problems.push(format!(
                "the query is {size} bytes without its comments, over the limit of {MAX_SIZE}"
            ));
        }
        for definition in &document.definitions {
            match definition {
                Definition::Fragment(fragment) => {
                    long_lists_in_set(&fragment.selection_set, &mut problems);
                }
                Definition::Operation(q::OperationDefinition::Query(query)) => {
                    for variable in &query.variable_definitions {
                        let name = format!("the variable `${}`", variable.name);
                        if let Some(default) = &variable.default_value {
                            long_lists(default, &name, &mut problems);
                        }
                        if let Some(Value::Array(items)) = variables.get(&variable.name)
                            && items.len() > MAX_LIST
                        {
                            problems.push(too_long(&name, items.len()));
                        }
                    }
                    long_lists_in_set(&query.selection_set, &mut problems);
                }
                Definition::Operation(q::OperationDefinition::SelectionSet(set)) => {
                    long_lists_in_set(set, &mut problems);
                }
                Definition::Operation(_) => {}
            }
        }
        let cost = cost(self, document);
        if cost > MAX_COST {
            // Shopify's own message: "Query has complexity of <cost>, which exceeds max
            // complexity of 30".
            problems.push(format!(
                "Query has complexity of {cost}, which exceeds max complexity of {MAX_COST}"
            ));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }
}

/// The calculated cost of the query of `document` (see [`Cost`]).
pub(super) fn cost<'q>(schema: &Schema, document: &'q q::Document<'q, String>) -> u32 {
    let mut fragments = Fragments::new();
    let mut roots = Vec::new();
    for definition in &document.definitions {
        match definition {
            Definition::Fragment(fragment) => {
                fragments.insert(fragment.name.as_str(), fragment);
            }
            Definition::Operation(q::OperationDefinition::Query(query)) => {
                roots.push(&query.selection_set);
            }
            Definition::Operation(q::OperationDefinition::SelectionSet(set)) => roots.push(set),
            Definition::Operation(_) => {}
        }
    }
    roots
        .into_iter()
        .map(|set| Cost::new(schema, &fragments).of(&[set], &schema.query))
        .max()
        .unwrap_or(0)
}

/// The bytes of `source` that are not in a comment: a `#` out of a string, to the end of its
/// line.
fn size_without_comments(source: &str) -> usize {
    let bytes = source.as_bytes();
    let (mut size, mut i) = (0, 0);
    while i < bytes.len() {
        if bytes[i] == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                i += 1;
            }
            continue;
        }
        let start = i;
        if bytes[i..].starts_with(b"\"\"\"") {
            i += 3;
            while i < bytes.len() && !bytes[i..].starts_with(b"\"\"\"") {
                i += if bytes[i..].starts_with(b"\\\"\"\"") {
                    4
                } else {
                    1
                };
            }
            i = (i + 3).min(bytes.len());
        } else if bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\n' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(bytes.len());
        } else {
            i += 1;
        }
        size += i - start;
    }
    size
}

fn too_long(what: &str, length: usize) -> String {
    format!("{what} has {length} elements, over the limit of {MAX_LIST} for a list")
}

fn long_lists_in_set(set: &SelectionSet<'_, String>, problems: &mut Vec<String>) {
    for selection in &set.items {
        match selection {
            Selection::Field(field) => {
                for (name, value) in &field.arguments {
                    long_lists(
                        value,
                        &format!("the argument `{name}` of `{}`", field.name),
                        problems,
                    );
                }
                long_lists_in_set(&field.selection_set, problems);
            }
            Selection::InlineFragment(fragment) => {
                long_lists_in_set(&fragment.selection_set, problems);
            }
            Selection::FragmentSpread(_) => {}
        }
    }
}

fn long_lists(value: &q::Value<'_, String>, what: &str, problems: &mut Vec<String>) {
    match value {
        q::Value::List(items) => {
            if items.len() > MAX_LIST {
                problems.push(too_long(what, items.len()));
            }
            for item in items {
                long_lists(item, what, problems);
            }
        }
        q::Value::Object(entries) => {
            for item in entries.values() {
                long_lists(item, what, problems);
            }
        }
        _ => {}
    }
}

/// The calculated cost of a query, from Shopify's table: `__typename` 0; a field that returns
/// a `Metafield` 3, and the fields of a `Metafield` 0; the `metaobject` root field 1;
/// `field(key:)` of a `Metaobject` 3; `hasAnyTag`, `hasTags`, `inAnyCollection` and
/// `inCollections` 3, and the fields of a `HasTagResponse` or a `CollectionMembership` 0; any
/// other leaf 1, and a field with fields to select 0. Each selection is counted once: the
/// selections of one name (or alias) under the same field are one. Where an object can be of
/// several types (a union, an interface), the cost is that of the costliest.
struct Cost<'s, 'f, 'q> {
    schema: &'s Schema,
    fragments: &'f Fragments<'q>,
    visiting: Vec<&'q str>,
}

impl<'s, 'f, 'q> Cost<'s, 'f, 'q> {
    fn new(schema: &'s Schema, fragments: &'f Fragments<'q>) -> Self {
        Cost {
            schema,
            fragments,
            visiting: Vec::new(),
        }
    }

    /// The cost of the selection sets `sets`, merged, on a value of the type `parent`.
    fn of(&mut self, sets: &[&'q SelectionSet<'q, String>], parent: &str) -> u32 {
        let objects = self.schema.objects_of(parent);
        objects
            .iter()
            .map(|object| {
                let mut fields = BTreeMap::new();
                for set in sets {
                    self.collect(set, object, &mut fields);
                }
                fields
                    .values()
                    .map(|group| self.field(group, object))
                    .sum::<u32>()
            })
            .max()
            .unwrap_or(0)
    }

    /// The fields of `set` that apply to the object type `object`, by response key.
    fn collect(
        &mut self,
        set: &'q SelectionSet<'q, String>,
        object: &str,
        fields: &mut BTreeMap<&'q str, Vec<&'q q::Field<'q, String>>>,
    ) {
        for selection in &set.items {
            match selection {
                Selection::Field(field) => fields
                    .entry(field.alias.as_deref().unwrap_or(&field.name))
                    .or_default()
                    .push(field),
                Selection::InlineFragment(fragment) => {
                    let applies = fragment.type_condition.as_ref().is_none_or(|condition| {
                        let q::TypeCondition::On(name) = condition;
                        self.schema.objects_of(name).contains(object)
                    });
                    if applies {
                        self.collect(&fragment.selection_set, object, fields);
                    }
                }
                Selection::FragmentSpread(spread) => {
                    let name = spread.fragment_name.as_str();
                    let Some(fragment) = self.fragments.get(name).copied() else {
                        continue;
                    };
                    let q::TypeCondition::On(condition) = &fragment.type_condition;
                    if self.visiting.contains(&name)
                        || !self.schema.objects_of(condition).contains(object)
                    {
                        continue;
                    }
                    self.visiting.push(name);
                    self.collect(&fragment.selection_set, object, fields);
                    self.visiting.pop();
                }
            }
        }
    }

    /// The cost of the selections `group` of one response key on the object type `object`.
    fn field(&mut self, group: &[&'q q::Field<'q, String>], object: &str) -> u32 {
        let name = group[0].name.as_str();
        let Some(definition) = self.schema.field(object, name) else {
            // `__typename`, or a field the schema check refuses.
            return 0;
        };
        let ty = definition.ty.base();
        let own = match (object, name) {
            ("Metafield" | "HasTagResponse" | "CollectionMembership", _) => 0,
            _ if ty == "Metafield" => 3,
            (_, "metaobject") if object == self.schema.query => 1,
            ("Metaobject", "field") => 3,
            (_, "hasAnyTag" | "hasTags" | "inAnyCollection" | "inCollections") => 3,
            _ if self.schema.has_composite(ty) => 0,
            _ => 1,
        };
        if !self.schema.has_composite(ty) {
            return own;
        }
        let sets: Vec<_> = group.iter().map(|field| &field.selection_set).collect();
        own + self.of(&sets, ty)
    }
}
