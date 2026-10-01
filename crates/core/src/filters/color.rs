//! Color filters. They answer in the notation of their input: hex in, hex out.

use slt_liquid::number::{float_to_s, to_number};
use slt_liquid::{Context, Environment, FilterArgs, Result, Value};

use crate::drops::color::{Color, ColorDrop, Notation};

fn parse(input: &Value) -> Option<Color> {
    match input.downcast::<ColorDrop>() {
        Some(drop) => Some(drop.0),
        None => Color::parse(&input.to_str()),
    }
}

fn number(value: &Value) -> f64 {
    to_number(value).to_f64()
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Applies a transformation to a color and formats the result, or returns nothing when the
/// input is not a color.
fn map(input: &Value, transform: impl FnOnce(Color) -> String) -> Result<Value> {
    Ok(parse(input).map_or(Value::Nil, |color| Value::from(transform(color))))
}

fn color_to_rgb(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    map(input, |color| color.to_rgb())
}

fn color_to_hsl(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    map(input, |color| color.to_hsl())
}

fn color_to_hex(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    map(input, |color| color.to_hex())
}

fn color_to_oklch(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    map(input, |color| color.to_oklch())
}

fn hex_to_rgba(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    map(input, |color| {
        let alpha = args.get(0).map_or(color.alpha, number);
        let alpha = float_to_s(round2(alpha));
        format!(
            "rgba({},{},{},{})",
            color.red.round(),
            color.green.round(),
            color.blue.round(),
            alpha.strip_suffix(".0").unwrap_or(&alpha)
        )
    })
}

fn color_extract(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let Some(color) = parse(input) else {
        return Ok(Value::Nil);
    };
    let (hue, saturation, lightness) = color.hsl();
    Ok(match args.at(0).to_str().as_ref() {
        "red" => Value::Int(color.red.round() as i64),
        "green" => Value::Int(color.green.round() as i64),
        "blue" => Value::Int(color.blue.round() as i64),
        "alpha" => Value::Float(round2(color.alpha)),
        "hue" => Value::Int(hue.round() as i64),
        "saturation" => Value::Int((saturation * 100.0).round() as i64),
        "lightness" => Value::Int((lightness * 100.0).round() as i64),
        _ => Value::Nil,
    })
}

fn color_brightness(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(parse(input).map_or(Value::Nil, |color| Value::Float(round2(color.brightness()))))
}

fn color_modify(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    map(input, |color| {
        let value = number(&args.at(1));
        let (hue, saturation, lightness) = color.hsl();
        let modified = match args.at(0).to_str().as_ref() {
            "red" => Color::rgb(value, color.green, color.blue, color.alpha, color.notation),
            "green" => Color::rgb(color.red, value, color.blue, color.alpha, color.notation),
            "blue" => Color::rgb(color.red, color.green, value, color.alpha, color.notation),
            "alpha" => Color::rgb(color.red, color.green, color.blue, value, color.notation),
            "hue" => color.with_hsl(value, saturation, lightness),
            "saturation" => color.with_hsl(hue, value / 100.0, lightness),
            "lightness" => color.with_hsl(hue, saturation, value / 100.0),
            _ => color,
        };
        modified.format()
    })
}

/// Shifts the saturation or lightness by percentage points.
fn shift(
    input: &Value,
    args: &FilterArgs,
    saturation_delta: f64,
    lightness_delta: f64,
) -> Result<Value> {
    map(input, |color| {
        let amount = number(&args.at(0)) / 100.0;
        let (hue, saturation, lightness) = color.hsl();
        color
            .with_hsl(
                hue,
                saturation + saturation_delta * amount,
                lightness + lightness_delta * amount,
            )
            .format()
    })
}

fn color_lighten(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    shift(input, args, 0.0, 1.0)
}

fn color_darken(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    shift(input, args, 0.0, -1.0)
}

fn color_saturate(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    shift(input, args, 1.0, 0.0)
}

fn color_desaturate(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    shift(input, args, -1.0, 0.0)
}

fn color_mix(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let (Some(color), Some(other)) = (parse(input), parse(&args.at(0))) else {
        return Ok(Value::Nil);
    };
    // The weight is the share of the input color, in percent.
    let weight = (number(&args.at(1)) / 100.0).clamp(0.0, 1.0);
    let blend = |a: f64, b: f64| a * weight + b * (1.0 - weight);
    let notation = if color.alpha < 1.0 || other.alpha < 1.0 {
        Notation::Rgb
    } else {
        color.notation
    };
    Ok(Value::from(
        Color::rgb(
            blend(color.red, other.red),
            blend(color.green, other.green),
            blend(color.blue, other.blue),
            blend(color.alpha, other.alpha),
            notation,
        )
        .format(),
    ))
}

fn color_contrast(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let (Some(color), Some(other)) = (parse(input), parse(&args.at(0))) else {
        return Ok(Value::Nil);
    };
    let (a, b) = (color.luminance(), other.luminance());
    let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
    Ok(Value::Float((ratio * 10.0).round() / 10.0))
}

fn color_difference(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let (Some(color), Some(other)) = (parse(input), parse(&args.at(0))) else {
        return Ok(Value::Nil);
    };
    let difference = (color.red.round() - other.red.round()).abs()
        + (color.green.round() - other.green.round()).abs()
        + (color.blue.round() - other.blue.round()).abs();
    Ok(Value::Int(difference as i64))
}

fn brightness_difference(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let (Some(color), Some(other)) = (parse(input), parse(&args.at(0))) else {
        return Ok(Value::Nil);
    };
    Ok(Value::Int(
        (color.brightness() - other.brightness()).abs().round() as i64,
    ))
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("color_to_rgb", color_to_rgb);
    env.register_filter("color_to_hsl", color_to_hsl);
    env.register_filter("color_to_hex", color_to_hex);
    env.register_filter("color_to_oklch", color_to_oklch);
    env.register_filter("hex_to_rgba", hex_to_rgba);
    env.register_filter("color_extract", color_extract);
    env.register_filter("color_brightness", color_brightness);
    env.register_filter("color_modify", color_modify);
    env.register_filter("color_lighten", color_lighten);
    env.register_filter("color_darken", color_darken);
    env.register_filter("color_saturate", color_saturate);
    env.register_filter("color_desaturate", color_desaturate);
    env.register_filter("color_mix", color_mix);
    env.register_filter("color_contrast", color_contrast);
    env.register_filter("color_difference", color_difference);
    env.register_filter("brightness_difference", brightness_difference);
}
