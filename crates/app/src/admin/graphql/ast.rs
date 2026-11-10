//! The tree of an executable GraphQL document: operations and fragments, as [`super::parse`]
//! reads them. Every node keeps where it starts, since graphql-ruby's errors point at fields,
//! arguments, values and directives; the fields of an input object keep their order and their
//! duplicates, which validation reports.

use crate::admin::schema::TypeRef;

/// A position in the source: 1-based line and column (in characters).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug)]
pub struct Document<'q> {
    pub definitions: Vec<Definition<'q>>,
}

impl<'q> Document<'q> {
    /// The operations, in order.
    pub fn operations(&self) -> impl Iterator<Item = &Operation<'q>> {
        self.definitions
            .iter()
            .filter_map(|definition| match definition {
                Definition::Operation(operation) => Some(operation),
                _ => None,
            })
    }

    /// The fragments, in order.
    pub fn fragments(&self) -> impl Iterator<Item = &Fragment<'q>> {
        self.definitions
            .iter()
            .filter_map(|definition| match definition {
                Definition::Fragment(fragment) => Some(fragment),
                _ => None,
            })
    }

    /// The first fragment of a name.
    pub fn fragment(&self, name: &str) -> Option<&Fragment<'q>> {
        self.fragments().find(|fragment| fragment.name == name)
    }
}

#[derive(Debug)]
pub enum Definition<'q> {
    Operation(Operation<'q>),
    Fragment(Fragment<'q>),
    /// A type system definition (`type`, `schema`, `extend`, ...), which a query may not
    /// contain: only where it is, for the error that says so.
    TypeSystem(Pos),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationKind {
    Query,
    Mutation,
    Subscription,
}

impl OperationKind {
    /// `query`, `mutation` or `subscription`.
    pub fn as_str(self) -> &'static str {
        match self {
            OperationKind::Query => "query",
            OperationKind::Mutation => "mutation",
            OperationKind::Subscription => "subscription",
        }
    }
}

#[derive(Debug)]
pub struct Operation<'q> {
    pub pos: Pos,
    pub kind: OperationKind,
    pub name: Option<&'q str>,
    pub variables: Vec<VariableDefinition<'q>>,
    pub directives: Vec<Directive<'q>>,
    pub selection_set: SelectionSet<'q>,
}

#[derive(Debug)]
pub struct VariableDefinition<'q> {
    pub pos: Pos,
    pub name: &'q str,
    pub ty: TypeRef,
    pub default_value: Option<Value<'q>>,
    pub directives: Vec<Directive<'q>>,
}

#[derive(Debug)]
pub struct Fragment<'q> {
    pub pos: Pos,
    pub name: &'q str,
    pub type_condition: &'q str,
    pub directives: Vec<Directive<'q>>,
    pub selection_set: SelectionSet<'q>,
}

#[derive(Debug, Default)]
pub struct SelectionSet<'q> {
    pub items: Vec<Selection<'q>>,
}

impl SelectionSet<'_> {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Debug)]
pub enum Selection<'q> {
    Field(Field<'q>),
    FragmentSpread(FragmentSpread<'q>),
    InlineFragment(InlineFragment<'q>),
}

#[derive(Debug)]
pub struct Field<'q> {
    pub pos: Pos,
    pub alias: Option<&'q str>,
    pub name: &'q str,
    pub arguments: Vec<Argument<'q>>,
    pub directives: Vec<Directive<'q>>,
    pub selection_set: SelectionSet<'q>,
}

impl<'q> Field<'q> {
    /// The key of the field in the response: its alias, or its name.
    pub fn response_key(&self) -> &'q str {
        self.alias.unwrap_or(self.name)
    }

    pub fn argument(&self, name: &str) -> Option<&Argument<'q>> {
        self.arguments.iter().find(|argument| argument.name == name)
    }
}

#[derive(Debug)]
pub struct FragmentSpread<'q> {
    pub pos: Pos,
    pub name: &'q str,
    pub directives: Vec<Directive<'q>>,
}

#[derive(Debug)]
pub struct InlineFragment<'q> {
    pub pos: Pos,
    pub type_condition: Option<&'q str>,
    pub directives: Vec<Directive<'q>>,
    pub selection_set: SelectionSet<'q>,
}

#[derive(Debug)]
pub struct Directive<'q> {
    pub pos: Pos,
    pub name: &'q str,
    pub arguments: Vec<Argument<'q>>,
}

/// An argument of a field or a directive, or a field of an input object value.
#[derive(Debug)]
pub struct Argument<'q> {
    pub pos: Pos,
    pub name: &'q str,
    pub value: Value<'q>,
}

#[derive(Debug)]
pub struct Value<'q> {
    pub pos: Pos,
    pub kind: ValueKind<'q>,
}

#[derive(Debug)]
pub enum ValueKind<'q> {
    Variable(&'q str),
    /// An integer, as written.
    Int(&'q str),
    /// A float, as written.
    Float(&'q str),
    String(String),
    Boolean(bool),
    Null,
    Enum(&'q str),
    List(Vec<Value<'q>>),
    Object(Vec<Argument<'q>>),
}

impl<'q> Value<'q> {
    pub fn is_null(&self) -> bool {
        matches!(self.kind, ValueKind::Null)
    }

    /// The variable the value is, if it is one.
    pub fn variable(&self) -> Option<&'q str> {
        match self.kind {
            ValueKind::Variable(name) => Some(name),
            _ => None,
        }
    }
}
