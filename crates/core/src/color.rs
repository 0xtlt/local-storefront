//! Colors: parsing, and conversion between formats. The theme's `color` object wraps them.

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

/// Rounds to a number of decimals, as the formats of a color and the `color` object do.
pub fn round_to(value: f64, decimals: i32) -> f64 {
    let factor = 10f64.powi(decimals);
    (value * factor).round() / factor
}

/// A rounded alpha or chroma as Ruby prints a float, as Shopify's color formats show it: `0.5`,
/// `1.0`. Those lie between 0 and 1 with two decimals at most, which `Display` writes out in
/// full, without an exponent.
fn float_text(value: f64) -> String {
    let text = value.to_string();
    if value.is_finite() && !text.contains('.') {
        format!("{text}.0")
    } else {
        text
    }
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

/// The CSS color names Shopify's color filters read, lowercase only as Shopify reads them
/// (`RED` is no color there): `{{ 'red' | color_to_rgb }}` is `rgb(255, 0, 0)` on
/// shopify.dev's playground (2026-11-16).
const NAMED: &[(&str, &str)] = &[
    ("aliceblue", "f0f8ff"),
    ("antiquewhite", "faebd7"),
    ("aqua", "00ffff"),
    ("aquamarine", "7fffd4"),
    ("azure", "f0ffff"),
    ("beige", "f5f5dc"),
    ("bisque", "ffe4c4"),
    ("black", "000000"),
    ("blanchedalmond", "ffebcd"),
    ("blue", "0000ff"),
    ("blueviolet", "8a2be2"),
    ("brown", "a52a2a"),
    ("burlywood", "deb887"),
    ("cadetblue", "5f9ea0"),
    ("chartreuse", "7fff00"),
    ("chocolate", "d2691e"),
    ("coral", "ff7f50"),
    ("cornflowerblue", "6495ed"),
    ("cornsilk", "fff8dc"),
    ("crimson", "dc143c"),
    ("cyan", "00ffff"),
    ("darkblue", "00008b"),
    ("darkcyan", "008b8b"),
    ("darkgoldenrod", "b8860b"),
    ("darkgray", "a9a9a9"),
    ("darkgreen", "006400"),
    ("darkgrey", "a9a9a9"),
    ("darkkhaki", "bdb76b"),
    ("darkmagenta", "8b008b"),
    ("darkolivegreen", "556b2f"),
    ("darkorange", "ff8c00"),
    ("darkorchid", "9932cc"),
    ("darkred", "8b0000"),
    ("darksalmon", "e9967a"),
    ("darkseagreen", "8fbc8f"),
    ("darkslateblue", "483d8b"),
    ("darkslategray", "2f4f4f"),
    ("darkslategrey", "2f4f4f"),
    ("darkturquoise", "00ced1"),
    ("darkviolet", "9400d3"),
    ("deeppink", "ff1493"),
    ("deepskyblue", "00bfff"),
    ("dimgray", "696969"),
    ("dimgrey", "696969"),
    ("dodgerblue", "1e90ff"),
    ("firebrick", "b22222"),
    ("floralwhite", "fffaf0"),
    ("forestgreen", "228b22"),
    ("fuchsia", "ff00ff"),
    ("gainsboro", "dcdcdc"),
    ("ghostwhite", "f8f8ff"),
    ("gold", "ffd700"),
    ("goldenrod", "daa520"),
    ("gray", "808080"),
    ("green", "008000"),
    ("greenyellow", "adff2f"),
    ("grey", "808080"),
    ("honeydew", "f0fff0"),
    ("hotpink", "ff69b4"),
    ("indianred", "cd5c5c"),
    ("indigo", "4b0082"),
    ("ivory", "fffff0"),
    ("khaki", "f0e68c"),
    ("lavender", "e6e6fa"),
    ("lavenderblush", "fff0f5"),
    ("lawngreen", "7cfc00"),
    ("lemonchiffon", "fffacd"),
    ("lightblue", "add8e6"),
    ("lightcoral", "f08080"),
    ("lightcyan", "e0ffff"),
    ("lightgoldenrodyellow", "fafad2"),
    ("lightgray", "d3d3d3"),
    ("lightgreen", "90ee90"),
    ("lightgrey", "d3d3d3"),
    ("lightpink", "ffb6c1"),
    ("lightsalmon", "ffa07a"),
    ("lightseagreen", "20b2aa"),
    ("lightskyblue", "87cefa"),
    ("lightslategray", "778899"),
    ("lightslategrey", "778899"),
    ("lightsteelblue", "b0c4de"),
    ("lightyellow", "ffffe0"),
    ("lime", "00ff00"),
    ("limegreen", "32cd32"),
    ("linen", "faf0e6"),
    ("magenta", "ff00ff"),
    ("maroon", "800000"),
    ("mediumaquamarine", "66cdaa"),
    ("mediumblue", "0000cd"),
    ("mediumorchid", "ba55d3"),
    ("mediumpurple", "9370db"),
    ("mediumseagreen", "3cb371"),
    ("mediumslateblue", "7b68ee"),
    ("mediumspringgreen", "00fa9a"),
    ("mediumturquoise", "48d1cc"),
    ("mediumvioletred", "c71585"),
    ("midnightblue", "191970"),
    ("mintcream", "f5fffa"),
    ("mistyrose", "ffe4e1"),
    ("moccasin", "ffe4b5"),
    ("navajowhite", "ffdead"),
    ("navy", "000080"),
    ("oldlace", "fdf5e6"),
    ("olive", "808000"),
    ("olivedrab", "6b8e23"),
    ("orange", "ffa500"),
    ("orangered", "ff4500"),
    ("orchid", "da70d6"),
    ("palegoldenrod", "eee8aa"),
    ("palegreen", "98fb98"),
    ("paleturquoise", "afeeee"),
    ("palevioletred", "db7093"),
    ("papayawhip", "ffefd5"),
    ("peachpuff", "ffdab9"),
    ("peru", "cd853f"),
    ("pink", "ffc0cb"),
    ("plum", "dda0dd"),
    ("powderblue", "b0e0e6"),
    ("purple", "800080"),
    ("rebeccapurple", "663399"),
    ("red", "ff0000"),
    ("rosybrown", "bc8f8f"),
    ("royalblue", "4169e1"),
    ("saddlebrown", "8b4513"),
    ("salmon", "fa8072"),
    ("sandybrown", "f4a460"),
    ("seagreen", "2e8b57"),
    ("seashell", "fff5ee"),
    ("sienna", "a0522d"),
    ("silver", "c0c0c0"),
    ("skyblue", "87ceeb"),
    ("slateblue", "6a5acd"),
    ("slategray", "708090"),
    ("slategrey", "708090"),
    ("snow", "fffafa"),
    ("springgreen", "00ff7f"),
    ("steelblue", "4682b4"),
    ("tan", "d2b48c"),
    ("teal", "008080"),
    ("thistle", "d8bfd8"),
    ("tomato", "ff6347"),
    ("turquoise", "40e0d0"),
    ("violet", "ee82ee"),
    ("wheat", "f5deb3"),
    ("white", "ffffff"),
    ("whitesmoke", "f5f5f5"),
    ("yellow", "ffff00"),
    ("yellowgreen", "9acd32"),
];

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

    /// A color as a store keeps one (a swatch, a color setting): hex, `rgb()` or `hsl()`, not
    /// a name, which Shopify refuses there.
    pub fn parse_value(input: &str) -> Option<Color> {
        let trimmed = input.trim();
        (trimmed.starts_with('#') || trimmed.contains('(')).then(|| Color::parse(trimmed))?
    }

    /// A color as Shopify's color filters read one: a value, or a CSS name.
    pub fn parse(input: &str) -> Option<Color> {
        let input = input.trim();
        // A name is the color of its hex value; `transparent` is black with no opacity.
        if input == "transparent" {
            return Some(Color::rgb(0.0, 0.0, 0.0, 0.0, Notation::Hex));
        }
        if let Some((_, hex)) = NAMED.iter().find(|(name, _)| *name == input) {
            return Color::parse(&format!("#{hex}"));
        }
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
                // Shopify reads no 4-digit hex (`#f00c` is no color there).
                3 => {
                    let expand = |index: usize| digit(index, 1).map(|value| value * 17.0);
                    Some(Color::rgb(
                        expand(0)?,
                        expand(1)?,
                        expand(2)?,
                        1.0,
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

    /// The red, green and blue channels, rounded to whole numbers.
    pub fn channels(&self) -> (u8, u8, u8) {
        (
            self.red.round() as u8,
            self.green.round() as u8,
            self.blue.round() as u8,
        )
    }

    fn alpha_text(&self) -> String {
        float_text(round_to(self.alpha, 2))
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
            float_text(round_to(chroma, 2)).trim_end_matches(".0"),
            hue.round(),
            float_text(round_to(self.alpha, 2))
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
    /// From the channels as they are, not rounded, as Shopify computes it:
    /// `hsl(100, 38%, 54%)` is 154.34.
    pub fn brightness(&self) -> f64 {
        (self.red * 299.0 + self.green * 587.0 + self.blue * 114.0) / 1000.0
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
