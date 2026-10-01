//! Font filters.

use slt_liquid::{Context, Environment, FilterArgs, Result, Value};

use super::site;
use crate::drops::font::{Font, FontDrop};

fn font_face(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let Some(drop) = input.downcast::<FontDrop>() else {
        return Ok(Value::empty_string());
    };
    let display = args
        .named("font_display")
        .map(|display| display.to_str().into_owned());
    Ok(Value::from(drop.font.face(site(ctx)?, display.as_deref())))
}

fn font_url(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let Some(drop) = input.downcast::<FontDrop>() else {
        return Ok(Value::Nil);
    };
    let format = args
        .get(0)
        .map(|format| format.to_str().into_owned())
        .unwrap_or_else(|| "woff2".to_string());
    Ok(Value::from(drop.font.url(site(ctx)?, &format)))
}

fn font_modify(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let Some(drop) = input.downcast::<FontDrop>() else {
        return Ok(Value::Nil);
    };
    let mut font: Font = drop.font.clone();
    let value = args.at(1).to_str().into_owned();
    match args.at(0).to_str().as_ref() {
        "weight" => {
            let weight = match value.as_str() {
                "normal" => Some(400),
                "bold" => Some(700),
                "bolder" => Some(match font.weight {
                    0..=300 => 400,
                    301..=500 => 700,
                    _ => 900,
                }),
                "lighter" => Some(match font.weight {
                    0..=500 => 100,
                    501..=700 => 400,
                    _ => 700,
                }),
                relative if relative.starts_with(['+', '-']) => relative
                    .parse::<i64>()
                    .ok()
                    .map(|delta| (i64::from(font.weight) + delta) as u32),
                absolute => absolute.parse::<u32>().ok(),
            };
            match weight.filter(|weight| (100..=900).contains(weight) && weight % 100 == 0) {
                Some(weight) => font.weight = weight,
                // A weight the family does not have yields nothing, so that `| default:` applies.
                None => return Ok(Value::Nil),
            }
        }
        "style" => match value.as_str() {
            "normal" => font.italic = false,
            "italic" | "oblique" => font.italic = true,
            _ => return Ok(Value::Nil),
        },
        _ => return Ok(Value::Nil),
    }
    Ok(FontDrop::value(site(ctx)?, font))
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("font_face", font_face);
    env.register_filter("font_url", font_url);
    env.register_filter("font_modify", font_modify);
}
