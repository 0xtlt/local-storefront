//! Array filters.

use std::cmp::Ordering;
use std::collections::HashSet;

use super::{check_arity, ruby_args};
use crate::context::Context;
use crate::environment::Environment;
use crate::error::{Error, Result};
use crate::number::{Number, to_number};
use crate::value::Value;
use crate::variable::FilterArgs;

/// The elements an array filter works on (Ruby Liquid's `InputIterator`): arrays are flattened,
/// a hash is a single element, `nil` is empty and any other value is a one-element list.
pub fn input_items(input: &Value) -> Vec<Value> {
    fn flatten(items: &[Value], out: &mut Vec<Value>) {
        for item in items {
            match item {
                Value::Array(nested) => flatten(nested, out),
                other => out.push(other.clone()),
            }
        }
    }
    match input {
        Value::Nil => Vec::new(),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            flatten(items, &mut out);
            out
        }
        Value::Range(from, to) => (*from..=*to).map(Value::Int).collect(),
        Value::Object(object) => match object.items() {
            Some(items) => items.as_ref().clone(),
            None => vec![input.clone()],
        },
        other => vec![other.clone()],
    }
}

/// Why `item[property]` could not be evaluated.
pub enum PropertyError {
    /// Ruby's `TypeError`: reported as "cannot select the property".
    Type,
    /// The item does not support `[]` at all (nil, booleans, floats): the filter returns nil.
    NoMethod,
}

/// `item[property]` as Ruby evaluates it for the different item types.
pub fn item_property(item: &Value, property: &Value) -> std::result::Result<Value, PropertyError> {
    match item {
        Value::Hash(map) => Ok(match property {
            Value::Str(key) => map.get(&**key).cloned().unwrap_or(Value::Nil),
            _ => Value::Nil,
        }),
        Value::Object(object) => Ok(match property {
            Value::Str(key) => object.get(key).unwrap_or(Value::Nil),
            Value::Int(index) => object.index(*index).unwrap_or(Value::Nil),
            _ => Value::Nil,
        }),
        // `"string"["sub"]` returns the substring when it is present.
        Value::Str(s) => match property {
            Value::Str(needle) => Ok(if s.contains(&**needle) {
                Value::Str(needle.clone())
            } else {
                Value::Nil
            }),
            Value::Int(index) => {
                let chars: Vec<char> = s.chars().collect();
                let len = chars.len() as i64;
                let index = if *index < 0 { index + len } else { *index };
                Ok(if index >= 0 && index < len {
                    Value::from(chars[index as usize].to_string())
                } else {
                    Value::Nil
                })
            }
            _ => Err(PropertyError::Type),
        },
        Value::Array(items) => match property {
            Value::Int(index) => {
                let len = items.len() as i64;
                let index = if *index < 0 { index + len } else { *index };
                Ok(if index >= 0 && index < len {
                    items[index as usize].clone()
                } else {
                    Value::Nil
                })
            }
            _ => Err(PropertyError::Type),
        },
        Value::Int(_) => match property {
            Value::Int(_) => Ok(Value::Int(0)),
            _ => Err(PropertyError::Type),
        },
        _ => Err(PropertyError::NoMethod),
    }
}

fn property_error(property: &Value) -> Error {
    Error::argument(format!(
        "cannot select the property '{}'",
        property.to_str()
    ))
}

/// Ruby truthiness, without unwrapping drops.
fn ruby_truthy(value: &Value) -> bool {
    !matches!(value, Value::Nil | Value::Bool(false))
}

fn join(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 0, 1)?;
    let glue = args
        .first()
        .map_or_else(|| " ".to_string(), |v| v.to_str().into_owned());
    let mut out = String::new();
    for (index, item) in input_items(input).iter().enumerate() {
        if index > 0 {
            out.push_str(&glue);
        }
        out.push_str(&item.to_str());
    }
    Ok(Value::from(out))
}

fn nil_safe_compare(a: &Value, b: &Value) -> Result<Ordering> {
    match a.ruby_cmp(b) {
        Some(ordering) => Ok(ordering),
        None if a.is_nil() => Ok(Ordering::Greater),
        None if b.is_nil() => Ok(Ordering::Less),
        None => Err(Error::argument("cannot sort values of incompatible types")),
    }
}

/// Ruby's `casecmp`: byte-wise comparison folding ASCII letters only.
fn nil_safe_casecmp(a: &Value, b: &Value) -> Result<Ordering> {
    Ok(match (a.is_nil(), b.is_nil()) {
        (false, false) => {
            let fold = |s: &str| {
                s.bytes()
                    .map(|b| b.to_ascii_lowercase())
                    .collect::<Vec<u8>>()
            };
            fold(&a.to_str()).cmp(&fold(&b.to_str()))
        }
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
    })
}

fn sort_with(
    input: &Value,
    args: &FilterArgs,
    compare: fn(&Value, &Value) -> Result<Ordering>,
) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 0, 1)?;
    let items = input_items(input);
    if items.is_empty() {
        return Ok(Value::array(Vec::new()));
    }
    let property = args.first().filter(|p| !p.is_nil());
    let mut keyed: Vec<(Value, Value)> = match property {
        None => items.into_iter().map(|item| (item.clone(), item)).collect(),
        Some(property) => {
            // Unless every element supports `[]` the reference returns nil.
            if items.iter().any(|item| {
                matches!(
                    item,
                    Value::Nil | Value::Bool(_) | Value::Float(_) | Value::Range(..)
                )
            }) {
                return Ok(Value::Nil);
            }
            let mut keyed = Vec::with_capacity(items.len());
            for item in items {
                match item_property(&item, property) {
                    Ok(key) => keyed.push((key, item)),
                    Err(PropertyError::Type) => return Err(property_error(property)),
                    // Not every element supports `[]`: the reference returns nil.
                    Err(PropertyError::NoMethod) => return Ok(Value::Nil),
                }
            }
            keyed
        }
    };
    let mut failure = None;
    keyed.sort_by(|(a, _), (b, _)| match compare(a, b) {
        Ok(ordering) => ordering,
        Err(error) => {
            failure.get_or_insert(error);
            Ordering::Equal
        }
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(Value::array(
            keyed.into_iter().map(|(_, item)| item).collect(),
        )),
    }
}

fn sort(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    sort_with(input, args, nil_safe_compare)
}

fn sort_natural(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    sort_with(input, args, nil_safe_casecmp)
}

enum Select {
    Where,
    Reject,
    Has,
    Find,
    FindIndex,
}

fn filter_array(input: &Value, args: &FilterArgs, mode: Select) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 2)?;
    let property = &args[0];
    let target = args.get(1).filter(|t| !t.is_nil());
    let items = input_items(input);
    if items.is_empty() {
        return Ok(match mode {
            Select::Where | Select::Reject => Value::array(Vec::new()),
            Select::Has => Value::Bool(false),
            Select::Find | Select::FindIndex => Value::Nil,
        });
    }
    let mut kept = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let value = match item_property(&item, property) {
            Ok(value) => value,
            Err(PropertyError::Type) => return Err(property_error(property)),
            Err(PropertyError::NoMethod) => return Ok(Value::Nil),
        };
        let matches = match target {
            None => ruby_truthy(&value),
            Some(target) => value.ruby_eq(target),
        };
        match mode {
            Select::Where if matches => kept.push(item),
            Select::Reject if !matches => kept.push(item),
            Select::Has if matches => return Ok(Value::Bool(true)),
            Select::Find if matches => return Ok(item),
            Select::FindIndex if matches => return Ok(Value::Int(index as i64)),
            _ => {}
        }
    }
    Ok(match mode {
        Select::Where | Select::Reject => Value::array(kept),
        Select::Has => Value::Bool(false),
        Select::Find | Select::FindIndex => Value::Nil,
    })
}

fn where_filter(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    filter_array(input, args, Select::Where)
}

fn reject(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    filter_array(input, args, Select::Reject)
}

fn has(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    filter_array(input, args, Select::Has)
}

fn find(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    filter_array(input, args, Select::Find)
}

fn find_index(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    filter_array(input, args, Select::FindIndex)
}

/// A key with the semantics of Ruby's `eql?`/`hash`: `1` and `1.0` are different.
fn uniq_key(value: &Value) -> String {
    match value.to_liquid_value() {
        Value::Int(i) => format!("i:{i}"),
        Value::Float(f) => format!("f:{f}"),
        Value::Str(s) => format!("s:{s}"),
        Value::Object(object) => match object.identity() {
            Some(identity) => format!("o:{identity}"),
            None => format!("p:{:p}", std::sync::Arc::as_ptr(&object)),
        },
        other => format!("v:{}", other.inspect()),
    }
}

fn uniq(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 0, 1)?;
    let items = input_items(input);
    let property = args.first().filter(|p| !p.is_nil());
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let key = match property {
            None => uniq_key(&item),
            Some(property) => match item_property(&item.to_liquid_value(), property) {
                Ok(value) => uniq_key(&value),
                Err(PropertyError::Type) => return Err(property_error(property)),
                Err(PropertyError::NoMethod) => return Ok(Value::Nil),
            },
        };
        if seen.insert(key) {
            out.push(item);
        }
    }
    Ok(Value::array(out))
}

fn reverse(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    check_arity(&ruby_args(args), 0, 0)?;
    let mut items = input_items(input);
    items.reverse();
    Ok(Value::array(items))
}

fn map(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 1)?;
    let property = &args[0];
    let mut out = Vec::new();
    for item in input_items(input) {
        out.push(match item_property(&item, property) {
            Ok(value) => value,
            Err(PropertyError::Type) => return Err(property_error(property)),
            Err(PropertyError::NoMethod) => Value::Nil,
        });
    }
    Ok(Value::array(out))
}

fn compact(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 0, 1)?;
    let items = input_items(input);
    let property = args.first().filter(|p| !p.is_nil());
    let mut out = Vec::new();
    for item in items {
        let keep = match property {
            None => !item.is_nil(),
            Some(property) => match item_property(&item, property) {
                Ok(value) => !value.is_nil(),
                Err(PropertyError::Type) => return Err(property_error(property)),
                Err(PropertyError::NoMethod) => return Ok(Value::Nil),
            },
        };
        if keep {
            out.push(item);
        }
    }
    Ok(Value::array(out))
}

fn concat(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 1)?;
    let Some(extra) = args[0].items() else {
        return Err(Error::argument("concat filter requires an array argument"));
    };
    let mut items = input_items(input);
    items.extend(extra.iter().cloned());
    Ok(Value::array(items))
}

fn first(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    check_arity(&ruby_args(args), 0, 0)?;
    Ok(match input {
        Value::Str(s) => s
            .chars()
            .next()
            .map_or_else(Value::empty_string, |c| Value::from(c.to_string())),
        Value::Array(items) => items.first().cloned().unwrap_or(Value::Nil),
        Value::Hash(map) => map.first().map_or(Value::Nil, |(key, value)| {
            Value::array(vec![Value::str(key), value.clone()])
        }),
        Value::Range(from, to) if from <= to => Value::Int(*from),
        Value::Object(object) => object
            .items()
            .and_then(|items| items.first().cloned())
            .unwrap_or(Value::Nil),
        _ => Value::Nil,
    })
}

fn last(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    check_arity(&ruby_args(args), 0, 0)?;
    Ok(match input {
        Value::Str(s) => s
            .chars()
            .last()
            .map_or_else(Value::empty_string, |c| Value::from(c.to_string())),
        Value::Array(items) => items.last().cloned().unwrap_or(Value::Nil),
        Value::Range(from, to) if from <= to => Value::Int(*to),
        Value::Object(object) => object
            .items()
            .and_then(|items| items.last().cloned())
            .unwrap_or(Value::Nil),
        _ => Value::Nil,
    })
}

fn sum(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 0, 1)?;
    let items = input_items(input);
    let property = args.first().filter(|p| !p.is_nil());
    let mut total = Value::Int(0);
    for item in items {
        let value = match property {
            None => item,
            Some(property) => match item_property(&item, property) {
                Ok(value) => value,
                Err(PropertyError::Type) => return Err(property_error(property)),
                Err(PropertyError::NoMethod) => Value::Int(0),
            },
        };
        // Nested arrays are summed element by element.
        for value in input_items(&value) {
            let number = match to_number(&value) {
                Number::Int(i) => Value::Int(i),
                other => Value::Float(other.to_f64()),
            };
            total = crate::number::apply(crate::number::Op::Add, &total, &number)?;
        }
    }
    Ok(total)
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("join", join);
    env.register_filter("sort", sort);
    env.register_filter("sort_natural", sort_natural);
    env.register_filter("where", where_filter);
    env.register_filter("reject", reject);
    env.register_filter("has", has);
    env.register_filter("find", find);
    env.register_filter("find_index", find_index);
    env.register_filter("uniq", uniq);
    env.register_filter("reverse", reverse);
    env.register_filter("map", map);
    env.register_filter("compact", compact);
    env.register_filter("concat", concat);
    env.register_filter("first", first);
    env.register_filter("last", last);
    env.register_filter("sum", sum);
}
