//! Colors: parsing, conversion between formats, and the `color` object.

use std::any::Any;
use std::borrow::Cow;

use slt_liquid::number::float_to_s;
use slt_liquid::{Object, Value};

/// The notation a color was written in. Color filters answer in the notation of their input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notation {
    Hex,
    Rgb,
    Hsl,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
    pub notation: Notation,
}

fn round_to(value: f64, decimals: i32) -> f64 {
    let factor = 10f64.powi(decimals);
    (value * factor).round() / factor
}

/// Parses the numbers inside `name(...)`, accepting commas, spaces, `/` and `%`.
fn components(body: &str) -> Vec<f64> {
    body.split([',', ' ', '/'])
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            part.trim_end_matches('%')
                .trim_end_matches("deg")
                .parse::<f64>()
                .ok()
        })
        .collect()
}

impl Color {
    pub fn rgb(red: f64, green: f64, blue: f64, alpha: f64, notation: Notation) -> Color {
        Color {
            red: red.clamp(0.0, 255.0),
            green: green.clamp(0.0, 255.0),
            blue: blue.clamp(0.0, 255.0),
            alpha: alpha.clamp(0.0, 1.0),
            notation,
        }
    }

    pub fn parse(input: &str) -> Option<Color> {
        let input = input.trim();
        if let Some(hex) = input.strip_prefix('#') {
            let digit = |index: usize, width: usize| {
                u8::from_str_radix(&hex[index..index + width], 16)
                    .ok()
                    .map(f64::from)
            };
            if !hex.is_ascii() {
                return None;
            }
            return match hex.len() {
                3 | 4 => {
                    let expand = |index: usize| digit(index, 1).map(|value| value * 17.0);
                    let alpha = if hex.len() == 4 {
                        expand(3)? / 255.0
                    } else {
                        1.0
                    };
                    Some(Color::rgb(
                        expand(0)?,
                        expand(1)?,
                        expand(2)?,
                        alpha,
                        Notation::Hex,
                    ))
                }
                6 | 8 => {
                    let alpha = if hex.len() == 8 {
                        digit(6, 2)? / 255.0
                    } else {
                        1.0
                    };
                    Some(Color::rgb(
                        digit(0, 2)?,
                        digit(2, 2)?,
                        digit(4, 2)?,
                        alpha,
                        Notation::Hex,
                    ))
                }
                _ => None,
            };
        }
        let (name, body) = input.strip_suffix(')')?.split_once('(')?;
        let values = components(body);
        match name.trim().to_ascii_lowercase().as_str() {
            "rgb" | "rgba" if values.len() >= 3 => Some(Color::rgb(
                values[0],
                values[1],
                values[2],
                values.get(3).copied().unwrap_or(1.0),
                Notation::Rgb,
            )),
            "hsl" | "hsla" if values.len() >= 3 => {
                let mut color = Color::from_hsl(
                    values[0],
                    values[1] / 100.0,
                    values[2] / 100.0,
                    values.get(3).copied().unwrap_or(1.0),
                );
                color.notation = Notation::Hsl;
                Some(color)
            }
            _ => None,
        }
    }

    /// Builds a color from a hue in degrees and saturation/lightness in `0..=1`.
    pub fn from_hsl(hue: f64, saturation: f64, lightness: f64, alpha: f64) -> Color {
        let saturation = saturation.clamp(0.0, 1.0);
        let lightness = lightness.clamp(0.0, 1.0);
        let hue = hue.rem_euclid(360.0);
        let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
        let x = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
        let m = lightness - chroma / 2.0;
        let (r, g, b) = match hue {
            h if h < 60.0 => (chroma, x, 0.0),
            h if h < 120.0 => (x, chroma, 0.0),
            h if h < 180.0 => (0.0, chroma, x),
            h if h < 240.0 => (0.0, x, chroma),
            h if h < 300.0 => (x, 0.0, chroma),
            _ => (chroma, 0.0, x),
        };
        Color::rgb(
            (r + m) * 255.0,
            (g + m) * 255.0,
            (b + m) * 255.0,
            alpha,
            Notation::Hsl,
        )
    }

    /// `(hue in degrees, saturation, lightness)`, the last two in `0..=1`.
    pub fn hsl(&self) -> (f64, f64, f64) {
        let (r, g, b) = (self.red / 255.0, self.green / 255.0, self.blue / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let lightness = (max + min) / 2.0;
        if delta == 0.0 {
            return (0.0, 0.0, lightness);
        }
        let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
        let hue = if max == r {
            60.0 * ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        (hue, saturation, lightness)
    }

    fn channels(&self) -> (u8, u8, u8) {
        (
            self.red.round() as u8,
            self.green.round() as u8,
            self.blue.round() as u8,
        )
    }

    fn alpha_text(&self) -> String {
        float_to_s(round_to(self.alpha, 2))
    }

    pub fn to_hex(&self) -> String {
        let (r, g, b) = self.channels();
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    pub fn to_rgb(&self) -> String {
        let (r, g, b) = self.channels();
        if self.alpha < 1.0 {
            format!("rgba({r}, {g}, {b}, {})", self.alpha_text())
        } else {
            format!("rgb({r}, {g}, {b})")
        }
    }

    pub fn to_hsl(&self) -> String {
        let (hue, saturation, lightness) = self.hsl();
        let (hue, saturation, lightness) = (
            hue.round(),
            (saturation * 100.0).round(),
            (lightness * 100.0).round(),
        );
        if self.alpha < 1.0 {
            format!(
                "hsla({hue}, {saturation}%, {lightness}%, {})",
                self.alpha_text()
            )
        } else {
            format!("hsl({hue}, {saturation}%, {lightness}%)")
        }
    }

    /// `(lightness 0..=1, chroma, hue in degrees)` in the OKLCH color space.
    pub fn oklch(&self) -> (f64, f64, f64) {
        let linear = |channel: f64| {
            let c = channel / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (linear(self.red), linear(self.green), linear(self.blue));
        let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
        let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
        let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
        let lightness = 0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s;
        let a = 1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s;
        let b = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s;
        let chroma = (a * a + b * b).sqrt();
        let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
        (lightness, chroma, hue)
    }

    pub fn to_oklch(&self) -> String {
        let (lightness, chroma, hue) = self.oklch();
        format!(
            "oklch({}% {} {} / {})",
            (lightness * 100.0).round(),
            float_to_s(round_to(chroma, 2)).trim_end_matches(".0"),
            hue.round(),
            float_to_s(round_to(self.alpha, 2))
        )
    }

    /// The color in the notation it was written in, which is what the color filters return.
    pub fn format(&self) -> String {
        match self.notation {
            Notation::Hex if self.alpha < 1.0 => self.to_rgb(),
            Notation::Hex => self.to_hex(),
            Notation::Rgb => self.to_rgb(),
            Notation::Hsl => self.to_hsl(),
        }
    }

    /// Perceived brightness per the W3C formula, `0..=255`.
    pub fn brightness(&self) -> f64 {
        let (r, g, b) = self.channels();
        (f64::from(r) * 299.0 + f64::from(g) * 587.0 + f64::from(b) * 114.0) / 1000.0
    }

    /// WCAG relative luminance.
    pub fn luminance(&self) -> f64 {
        let channel = |value: f64| {
            let c = value.round() / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(self.red) + 0.7152 * channel(self.green) + 0.0722 * channel(self.blue)
    }

    pub fn with_hsl(&self, hue: f64, saturation: f64, lightness: f64) -> Color {
        let mut color = Color::from_hsl(hue, saturation, lightness, self.alpha);
        color.notation = self.notation;
        color
    }
}

/// The `color` object, as returned by color settings.
pub struct ColorDrop(pub Color);

impl ColorDrop {
    pub fn value(color: Color) -> Value {
        Value::object(ColorDrop(color))
    }
}

impl Object for ColorDrop {
    fn type_name(&self) -> &str {
        "color"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let color = &self.0;
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
        Some(Value::from(self.0.format()))
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Owned(self.0.format())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats() {
        let color = Color::parse("#EA5AB9").unwrap();
        assert_eq!(color.to_hex(), "#ea5ab9");
        assert_eq!(color.to_rgb(), "rgb(234, 90, 185)");
        assert_eq!(color.to_hsl(), "hsl(320, 77%, 64%)");
        assert_eq!(color.to_oklch(), "oklch(68% 0.2 343 / 1.0)");
        assert_eq!(
            Color::parse("rgba(232, 0, 176, 0.75)").unwrap().format(),
            "rgba(232, 0, 176, 0.75)"
        );
        assert_eq!(
            Color::parse("#00000026").unwrap().format(),
            "rgba(0, 0, 0, 0.15)"
        );
        assert_eq!(Color::parse("#fff").unwrap().to_hex(), "#ffffff");
        assert!(Color::parse("nope").is_none());
    }

    #[test]
    fn modifies_lightness_like_shopify() {
        let color = Color::parse("#EA5AB9").unwrap();
        let (h, s, l) = color.hsl();
        assert_eq!(color.with_hsl(h, s, l + 0.30).format(), "#fbe2f3");
        assert_eq!(color.with_hsl(h, s, l - 0.30).format(), "#98136b");
        assert_eq!(color.with_hsl(h, s + 0.30, l).format(), "#ff45c0");
        assert_eq!(color.with_hsl(h, s - 0.30, l).format(), "#ce76b0");
        assert_eq!(round_to(color.brightness(), 2), 143.89);
    }
}
