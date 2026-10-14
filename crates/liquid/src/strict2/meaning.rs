//! Whether two expressions mean the same, whatever markup they were written with.

use crate::expr::{Expr, Lookup, LookupName};
use crate::value::Value;

/// The keys that fall back to a method (`size`, `first`, `last`) when written `.key`.
const COMMANDS: [&str; 3] = ["size", "first", "last"];

/// An expression without the differences of writing that do not change what it renders:
/// `['a']`, `self.a` and `a` read the same variable, `x['b']` is `x.b`, and a lookup of nothing
/// is `nil`. This takes it that no template assigns a variable named `self`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Meaning {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    IntRange(i64, i64),
    Range(Box<Meaning>, Box<Meaning>),
    /// `blank` in a condition.
    Blank,
    /// `empty` in a condition.
    Empty,
    Lookup(Name, Vec<Segment>),
    /// A value that parsing does not produce.
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Name {
    Static(String),
    /// A variable whose name is computed, written `self[expr]`.
    Dynamic(Box<Meaning>),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Segment {
    /// `.key`, or `['key']` when `key` is not one of the method keys.
    Key(String),
    Index(Meaning),
}

impl From<&Expr> for Meaning {
    fn from(expr: &Expr) -> Meaning {
        match expr {
            Expr::Literal(value) => match value {
                Value::Nil => Meaning::Nil,
                Value::Bool(b) => Meaning::Bool(*b),
                Value::Int(i) => Meaning::Int(*i),
                Value::Float(f) => Meaning::Float(*f),
                Value::Str(s) => Meaning::Str(s.to_string()),
                Value::Range(from, to) => Meaning::IntRange(*from, *to),
                other => Meaning::Other(format!("{other:?}")),
            },
            Expr::Range(from, to) => {
                Meaning::Range(Box::new(from.as_ref().into()), Box::new(to.as_ref().into()))
            }
            Expr::Blank => Meaning::Blank,
            Expr::Empty => Meaning::Empty,
            Expr::Lookup(lookup) => {
                let mut segments: Vec<Segment> = lookup
                    .lookups
                    .iter()
                    .map(|lookup| match lookup {
                        Lookup::Key { name, .. } => Segment::Key(name.clone()),
                        Lookup::Index(index) => match Meaning::from(index) {
                            Meaning::Str(key) if !COMMANDS.contains(&key.as_str()) => {
                                Segment::Key(key)
                            }
                            index => Segment::Index(index),
                        },
                    })
                    .collect();
                let mut name = match &lookup.name {
                    LookupName::Missing => return Meaning::Nil,
                    LookupName::Static(name) => Name::Static(name.clone()),
                    LookupName::Dynamic(expr) => match Meaning::from(expr.as_ref()) {
                        Meaning::Str(name) => Name::Static(name),
                        expr => Name::Dynamic(Box::new(expr)),
                    },
                };
                // `self.a` and `self[expr]` read a variable.
                if name == Name::Static("self".to_string()) {
                    match segments.first() {
                        Some(Segment::Key(key)) if !COMMANDS.contains(&key.as_str()) => {
                            name = Name::Static(key.clone());
                            segments.remove(0);
                        }
                        Some(Segment::Index(index)) => {
                            name = Name::Dynamic(Box::new(index.clone()));
                            segments.remove(0);
                        }
                        _ => {}
                    }
                }
                Meaning::Lookup(name, segments)
            }
        }
    }
}

/// A key that is the same for two literals that `==` finds equal, `None` for an expression that
/// is not a literal (or is `blank`/`empty`, which equal several values).
pub(crate) fn literal_key(expr: &Expr) -> Option<String> {
    Some(match Meaning::from(expr) {
        Meaning::Nil => "nil".to_string(),
        Meaning::Bool(b) => format!("bool:{b}"),
        Meaning::Int(i) => format!("number:{}", i as f64),
        Meaning::Float(f) => format!("number:{f}"),
        Meaning::Str(s) => format!("string:{s}"),
        _ => return None,
    })
}

/// Expressions are equal when they mean the same.
impl PartialEq for Expr {
    fn eq(&self, other: &Expr) -> bool {
        Meaning::from(self) == Meaning::from(other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: &str, b: &str) -> bool {
        Expr::parse(a) == Expr::parse(b)
    }

    #[test]
    fn writings_of_the_same_lookup_are_equal() {
        assert!(same("['a'].b", "a.b"));
        assert!(same("self.a", "a"));
        assert!(same("self['a']", "a"));
        assert!(same("[x]", "self[x]"));
        assert!(same("a['b']", "a.b"));
        assert!(same("1.2.3", "1.2"));
        assert!(!same("a['size']", "a.size"));
        assert!(!same("a", "b"));
        assert!(!same("1", "1.0"));
    }
}
