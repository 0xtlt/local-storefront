//! The `date` filter.

use super::{check_arity, ruby_args};
use crate::context::Context;
use crate::environment::Environment;
use crate::error::{Error, Result};
use crate::time::{strftime, to_time};
use crate::value::Value;
use crate::variable::FilterArgs;

fn date(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 1)?;
    let format = args[0].to_str();
    if format.is_empty() {
        return Ok(input.clone());
    }
    match to_time(input, ctx) {
        Some(time) => strftime(&time, &format)
            .map(Value::from)
            .map_err(Error::argument),
        None => Ok(input.clone()),
    }
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("date", date);
}
