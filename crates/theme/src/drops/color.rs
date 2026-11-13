//! The `color` object: a [`Color`] as Liquid sees it.

use std::any::Any;
use std::borrow::Cow;

use lsf_core::color::{Color, round_to};
use lsf_liquid::number::float_to_s;
use lsf_liquid::{Object, Value};

/// The `color` object, as returned by color settings.
pub struct ColorDrop {
    pub color: Color,
    /// What it prints: the color in the notation it was written in.
    printed: String,
}

impl ColorDrop {
    /// The color a setting, a metafield or a swatch stores, or `None` when `text` is not one.
    /// It prints as stored when that is the color Shopify prints in another case
    /// (`#FF00aa`): Shopify keeps the case it was given.
    pub fn parse(text: &str) -> Option<Value> {
        let color = Color::parse(text)?;
        let formatted = color.format();
        let printed = if text.trim().eq_ignore_ascii_case(&formatted) {
            text.trim().to_string()
        } else {
            formatted
        };
        Some(Value::object(ColorDrop { color, printed }))
    }
}

impl Object for ColorDrop {
    fn type_name(&self) -> &str {
        "color"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let color = &self.color;
        let (r, g, b) = color.channels();
        let (hue, saturation, lightness) = color.hsl();
        Some(match key {
            "red" => Value::Int(i64::from(r)),
            "green" => Value::Int(i64::from(g)),
            "blue" => Value::Int(i64::from(b)),
            "alpha" => Value::Float(round_to(color.alpha, 2)),
            "hue" => Value::Int(hue.round() as i64),
            "saturation" => Value::Int((saturation * 100.0).round() as i64),
            "lightness" => Value::Int((lightness * 100.0).round() as i64),
            "rgb" => Value::from(format!("{r} {g} {b}")),
            "rgba" => Value::from(format!(
                "{r} {g} {b} / {}",
                float_to_s(round_to(color.alpha, 2))
            )),
            "oklch" | "oklcha" => Value::from(color.to_oklch()),
            "chroma" => Value::Float(round_to(color.oklch().1, 2)),
            "color_space" => Value::str("srgb"),
            _ => return None,
        })
    }

    /// A color prints, compares and feeds filters as its CSS string.
    fn to_value(&self) -> Option<Value> {
        Some(Value::from(&self.printed))
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.printed)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use lsf_core::color::Notation;

    use super::*;

    /// lsf-core prints the alpha and the chroma of a color without Liquid: they have to read as
    /// the `color` object, which prints them as Liquid prints a float, writes them.
    #[test]
    fn prints_alpha_and_chroma_as_liquid_prints_a_float() {
        let alphas = (0..=100).map(|percent| f64::from(percent) / 100.0);
        for alpha in alphas.chain([-0.0, 0.004, 0.005, 0.995, 1.0 / 3.0, f64::NAN]) {
            let color = Color::rgb(234.0, 90.0, 185.0, alpha, Notation::Rgb);
            let printed = float_to_s(round_to(color.alpha, 2));
            if color.alpha < 1.0 {
                assert_eq!(color.to_rgb(), format!("rgba(234, 90, 185, {printed})"));
            }
            assert!(
                color.to_oklch().ends_with(&format!(" / {printed})")),
                "{alpha}"
            );
        }
        for hex in [
            "#000000", "#ffffff", "#ea5ab9", "#00ff00", "#0000ff", "#808080", "#123456",
        ] {
            let color = Color::parse(hex).unwrap();
            let chroma = float_to_s(round_to(color.oklch().1, 2));
            let chroma = chroma.trim_end_matches(".0");
            assert!(color.to_oklch().contains(&format!("% {chroma} ")), "{hex}");
        }
    }
}
