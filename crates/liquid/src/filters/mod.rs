//! The filters of standard Liquid.

mod array;
mod date;
mod math;
mod string;

pub use array::{input_items, item_property};
pub use string::{escape_html, url_encode};

use crate::context::Context;
use crate::environment::Environment;
use crate::error::{Error, Result};
use crate::value::{Hash, Value};
use crate::variable::FilterArgs;

/// The arguments as Ruby sees them: keyword arguments become one trailing hash.
pub(crate) fn ruby_args(args: &FilterArgs) -> Vec<Value> {
    let mut out = args.positional.clone();
    if !args.named.is_empty() {
        let hash: Hash = args.named.iter().cloned().collect();
        out.push(Value::hash(hash));
    }
    out
}

/// Checks the number of arguments and reports mismatches the way Ruby does, counting the
/// filter input as the first argument.
pub(crate) fn check_arity(args: &[Value], min: usize, max: usize) -> Result<()> {
    if args.len() < min || args.len() > max {
        let expected = if min == max {
            (min + 1).to_string()
        } else {
            format!("{}..{}", min + 1, max + 1)
        };
        return Err(Error::wrong_arity(args.len() + 1, &expected));
    }
    Ok(())
}

fn size(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    check_arity(&ruby_args(args), 0, 0)?;
    Ok(Value::Int(match input {
        Value::Object(object) => object.size().unwrap_or(0) as i64,
        other => other.size().unwrap_or(0) as i64,
    }))
}

fn default(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    // `default(input, default_value = '', options = {})`: keyword arguments arrive as one
    // trailing hash, so without a positional argument they *are* the default value.
    let args = ruby_args(args);
    check_arity(&args, 0, 2)?;
    let allow_false = args
        .get(1)
        .and_then(Value::as_hash)
        .and_then(|options| options.get("allow_false"))
        .is_some_and(Value::is_truthy);
    let use_default = if allow_false {
        input.is_nil()
    } else {
        !input.is_truthy()
    } || input.is_empty().unwrap_or(false);
    Ok(if use_default {
        args.first().cloned().unwrap_or_else(Value::empty_string)
    } else {
        input.clone()
    })
}

pub(crate) fn register(env: &mut Environment) {
    env.register_filter("size", size);
    env.register_filter("default", default);
    string::register(env);
    math::register(env);
    array::register(env);
    date::register(env);
}
