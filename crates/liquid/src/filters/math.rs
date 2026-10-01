//! Arithmetic filters.

use std::cmp::Ordering;

use super::{check_arity, ruby_args};
use crate::context::Context;
use crate::environment::Environment;
use crate::error::Result;
use crate::number::{self, Op, to_number};
use crate::value::Value;
use crate::variable::FilterArgs;

fn binary(op: Op, input: &Value, args: &FilterArgs) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 1)?;
    number::apply(op, input, &args[0])
}

fn plus(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    binary(Op::Add, input, args)
}

fn minus(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    binary(Op::Sub, input, args)
}

fn times(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    binary(Op::Mul, input, args)
}

fn divided_by(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    binary(Op::Div, input, args)
}

fn modulo(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    binary(Op::Mod, input, args)
}

fn abs(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    check_arity(&ruby_args(args), 0, 0)?;
    Ok(number::abs(input))
}

fn round(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 0, 1)?;
    Ok(number::round(input, args.first().unwrap_or(&Value::Int(0))))
}

fn ceil(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    check_arity(&ruby_args(args), 0, 0)?;
    Ok(number::ceil(input))
}

fn floor(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    check_arity(&ruby_args(args), 0, 0)?;
    Ok(number::floor(input))
}

fn clamp(input: &Value, args: &FilterArgs, replace_when: Ordering) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 1)?;
    let bound = to_number(&args[0]);
    let value = to_number(input);
    let result = if bound.partial_cmp(value) == Some(replace_when) {
        bound
    } else {
        value
    };
    Ok(result.into_value())
}

fn at_least(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    clamp(input, args, Ordering::Greater)
}

fn at_most(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    clamp(input, args, Ordering::Less)
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("plus", plus);
    env.register_filter("minus", minus);
    env.register_filter("times", times);
    env.register_filter("divided_by", divided_by);
    env.register_filter("modulo", modulo);
    env.register_filter("abs", abs);
    env.register_filter("round", round);
    env.register_filter("ceil", ceil);
    env.register_filter("floor", floor);
    env.register_filter("at_least", at_least);
    env.register_filter("at_most", at_most);
}
