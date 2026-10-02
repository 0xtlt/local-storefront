//! The `font` object returned by `font_picker` settings.
//!
//! A font handle such as `work_sans_n6` names a family (`work_sans`), a style (`n`ormal or
//! `i`talic) and a weight (`6` → 600). Shopify knows the real name and fallback of every font
//! in its library; here the common ones are listed and the rest are derived from the handle.

use std::any::Any;

use lsf_liquid::{Object, Value};

use super::SiteRef;

#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    /// The family part of the handle, e.g. `work_sans`.
    pub family_handle: String,
    pub weight: u32,
    pub italic: bool,
}

/// `(handle, CSS family, fallback families, is a system font)`.
const KNOWN: &[(&str, &str, &str, bool)] = &[
    ("helvetica", "Helvetica", "Arial, sans-serif", true),
    ("arial", "Arial", "Helvetica, sans-serif", true),
    (
        "times_new_roman",
        "\"Times New Roman\"",
        "Times, serif",
        true,
    ),
    ("garamond", "Garamond", "Baskerville, Caslon, serif", true),
    ("georgia", "Georgia", "serif", true),
    ("courier_new", "\"Courier New\"", "Courier, monospace", true),
    (
        "lucida_grande",
        "\"Lucida Grande\"",
        "\"Lucida Sans Unicode\", \"Lucida Sans\", Lucida, Helvetica, Arial, sans-serif",
        true,
    ),
    (
        "monaco",
        "Monaco",
        "\"Lucida Console\", \"DejaVu Sans Mono\", monospace",
        true,
    ),
    (
        "palatino",
        "Palatino",
        "\"Palatino Linotype\", \"Book Antiqua\", serif",
        true,
    ),
    ("tahoma", "Tahoma", "Verdana, Segoe, sans-serif", true),
    ("trebuchet_ms", "\"Trebuchet MS\"", "sans-serif", true),
    ("verdana", "Verdana", "Geneva, sans-serif", true),
    (
        "mono",
        "Menlo",
        "Consolas, Monaco, \"Liberation Mono\", \"Lucida Console\", monospace, \"Apple Color Emoji\", \"Segoe UI Emoji\", \"Segoe UI Symbol\"",
        true,
    ),
    (
        "serif",
        "Iowan Old Style",
        "\"Apple Garamond\", Baskerville, \"Times New Roman\", \"Droid Serif\", Times, \"Source Serif Pro\", serif, \"Apple Color Emoji\", \"Segoe UI Emoji\", \"Segoe UI Symbol\"",
        true,
    ),
    (
        "sans-serif",
        "-apple-system",
        "BlinkMacSystemFont, \"Segoe UI\", Roboto, Ubuntu, \"Helvetica Neue\", sans-serif, \"Apple Color Emoji\", \"Segoe UI Emoji\", \"Segoe UI Symbol\"",
        true,
    ),
    ("system_ui", "system-ui", "sans-serif", true),
    ("playfair_display", "\"Playfair Display\"", "serif", false),
    ("lora", "Lora", "serif", false),
    ("merriweather", "Merriweather", "serif", false),
    ("libre_baskerville", "\"Libre Baskerville\"", "serif", false),
    ("cormorant", "Cormorant", "serif", false),
    ("crimson_text", "\"Crimson Text\"", "serif", false),
    ("eb_garamond", "\"EB Garamond\"", "serif", false),
    ("dm_serif_display", "\"DM Serif Display\"", "serif", false),
    ("source_serif_pro", "\"Source Serif Pro\"", "serif", false),
    ("noto_serif", "\"Noto Serif\"", "serif", false),
    ("pt_serif", "\"PT Serif\"", "serif", false),
    ("bitter", "Bitter", "serif", false),
    ("roboto_mono", "\"Roboto Mono\"", "monospace", false),
    ("ibm_plex_mono", "\"IBM Plex Mono\"", "monospace", false),
    ("space_mono", "\"Space Mono\"", "monospace", false),
    ("source_code_pro", "\"Source Code Pro\"", "monospace", false),
];

impl Font {
    /// Parses a font handle. Unknown shapes are treated as a family at weight 400.
    pub fn parse(handle: &str) -> Font {
        if let Some((family, variant)) = handle.rsplit_once('_') {
            let mut chars = variant.chars();
            if let (Some(style @ ('n' | 'i')), Some(weight), None) = (
                chars.next(),
                chars.next().and_then(|c| c.to_digit(10)),
                chars.next(),
            ) {
                return Font {
                    family_handle: family.to_string(),
                    weight: weight * 100,
                    italic: style == 'i',
                };
            }
        }
        Font {
            family_handle: handle.to_string(),
            weight: 400,
            italic: false,
        }
    }

    pub fn handle(&self) -> String {
        format!(
            "{}_{}{}",
            self.family_handle,
            if self.italic { 'i' } else { 'n' },
            self.weight / 100
        )
    }

    fn known(&self) -> Option<&'static (&'static str, &'static str, &'static str, bool)> {
        KNOWN
            .iter()
            .find(|(handle, _, _, _)| *handle == self.family_handle)
    }

    /// The CSS family name, quoted when it needs to be.
    pub fn family(&self) -> String {
        if let Some((_, family, _, _)) = self.known() {
            return (*family).to_string();
        }
        let name = self
            .family_handle
            .split('_')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    // Short all-letter words are usually acronyms: `dm_sans` → `DM Sans`.
                    Some(first) if word.len() <= 2 => first
                        .to_uppercase()
                        .chain(chars.flat_map(char::to_uppercase))
                        .collect::<String>(),
                    Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        if name.contains(' ') {
            format!("\"{name}\"")
        } else {
            name
        }
    }

    pub fn fallback_families(&self) -> &'static str {
        self.known()
            .map_or("sans-serif", |(_, _, fallback, _)| fallback)
    }

    pub fn is_system(&self) -> bool {
        self.known().is_some_and(|(_, _, _, system)| *system)
    }

    pub fn style(&self) -> &'static str {
        if self.italic { "italic" } else { "normal" }
    }

    /// The URL the font file would be served from. Fonts are not bundled: drop the files into
    /// `files/fonts/` of the data directory to serve them, otherwise the fallback is used.
    pub fn url(&self, site: &SiteRef, format: &str) -> String {
        format!(
            "{}/cdn/fonts/{}/{}.{format}",
            crate::urls::cdn_origin(site),
            self.family_handle,
            self.handle()
        )
    }

    /// The `@font-face` rule, empty for system fonts.
    pub fn face(&self, site: &SiteRef, font_display: Option<&str>) -> String {
        if self.is_system() {
            return String::new();
        }
        let display = font_display
            .map(|display| format!("  font-display: {display};\n"))
            .unwrap_or_default();
        format!(
            "@font-face {{\n  font-family: {};\n  font-weight: {};\n  font-style: {};\n{display}  src: url(\"{}\") format(\"woff2\"),\n       url(\"{}\") format(\"woff\");\n}}\n",
            self.family(),
            self.weight,
            self.style(),
            self.url(site, "woff2"),
            self.url(site, "woff"),
        )
    }
}

pub struct FontDrop {
    pub site: SiteRef,
    pub font: Font,
}

impl FontDrop {
    pub fn value(site: &SiteRef, font: Font) -> Value {
        Value::object(FontDrop {
            site: site.clone(),
            font,
        })
    }
}

impl Object for FontDrop {
    fn type_name(&self) -> &str {
        "font"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let font = &self.font;
        Some(match key {
            "family" => Value::from(font.family()),
            "fallback_families" => Value::str(font.fallback_families()),
            "weight" => Value::from(font.weight),
            "style" => Value::str(font.style()),
            "system?" => Value::Bool(font.is_system()),
            "baseline_ratio" => Value::Float(0.0),
            "variants" => Value::array(
                [false, true]
                    .into_iter()
                    .flat_map(|italic| {
                        (1..=9).map(move |weight| Font {
                            family_handle: font.family_handle.clone(),
                            weight: weight * 100,
                            italic,
                        })
                    })
                    .map(|variant| FontDrop::value(&self.site, variant))
                    .collect(),
            ),
            _ => return None,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("font:{}", self.font.handle()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_handles() {
        let font = Font::parse("work_sans_i6");
        assert_eq!(font.family_handle, "work_sans");
        assert_eq!((font.weight, font.italic), (600, true));
        assert_eq!(font.family(), "\"Work Sans\"");
        assert_eq!(font.handle(), "work_sans_i6");
        assert_eq!(Font::parse("inter_n4").family(), "Inter");
        assert_eq!(Font::parse("dm_sans_n4").family(), "\"DM Sans\"");
        assert!(Font::parse("helvetica_n4").is_system());
        assert_eq!(Font::parse("sans-serif").family_handle, "sans-serif");
    }
}
