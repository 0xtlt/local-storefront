//! The `theme_settings` of the store data, checked against the settings the theme declares in
//! `config/settings_schema.json`: an id the theme does not have, or a value its setting does not
//! take, would otherwise do nothing.

use indexmap::IndexMap;
use serde_json::Value as Json;

use crate::diagnostics::Diagnostics;
use crate::util::closest_match;

use super::build::Origin;

/// A setting of `config/settings_schema.json`.
struct Setting<'a> {
    kind: &'a str,
    definition: &'a Json,
}

/// The settings of a `config/settings_schema.json`, by id.
fn settings(schema: &Json) -> IndexMap<&str, Setting<'_>> {
    let mut settings = IndexMap::new();
    for group in schema.as_array().into_iter().flatten() {
        for definition in group
            .get("settings")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(id), Some(kind)) = (
                definition.get("id").and_then(Json::as_str),
                definition.get("type").and_then(Json::as_str),
            ) else {
                continue;
            };
            settings.insert(id, Setting { kind, definition });
        }
    }
    settings
}

fn number(value: Option<&Json>) -> Option<f64> {
    value.and_then(|value| match value {
        Json::Number(number) => number.as_f64(),
        Json::String(text) => text.parse().ok(),
        _ => None,
    })
}

/// What is wrong with a value of a setting, with a hint.
fn problem(setting: &Setting<'_>, value: &Json) -> Option<(String, String)> {
    let definition = setting.definition;
    let kind = setting.kind;
    // `null` clears a setting, whatever its type.
    if value.is_null() {
        return None;
    }
    match kind {
        "checkbox" => (!value.is_boolean()).then(|| {
            (
                format!("a checkbox setting is true or false, not {value}"),
                "write true or false, without quotes".to_string(),
            )
        }),
        "range" | "number" => {
            let Some(found) = number(Some(value)).filter(|_| value.is_number()) else {
                return Some((
                    format!("a {kind} setting is a number, not {value}"),
                    "write a number, without quotes".to_string(),
                ));
            };
            let min = number(definition.get("min"));
            let max = number(definition.get("max"));
            let outside = min.is_some_and(|min| found < min) || max.is_some_and(|max| found > max);
            outside.then(|| {
                (
                    format!("{found} is outside the range of the setting"),
                    format!(
                        "the theme takes {} to {}",
                        min.map_or("any".to_string(), |min| min.to_string()),
                        max.map_or("any".to_string(), |max| max.to_string())
                    ),
                )
            })
        }
        "select" | "radio" => {
            let options: Vec<&str> = definition
                .get("options")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| option.get("value").and_then(Json::as_str))
                .collect();
            let text = value.as_str();
            if options.is_empty() || text.is_some_and(|text| options.contains(&text)) {
                return None;
            }
            let shown = text.map_or_else(|| value.to_string(), |text| format!("\"{text}\""));
            let hint = match text.and_then(|text| closest_match(text, options.iter().copied())) {
                Some(close) => format!("did you mean \"{close}\"?"),
                None => format!(
                    "the setting takes {}",
                    options
                        .iter()
                        .map(|option| format!("\"{option}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            Some((format!("{shown} is not an option of the setting"), hint))
        }
        "color" => {
            let valid = value.as_str().is_some_and(|text| {
                // A setting can name another: `{{ settings.accent }}`.
                text.is_empty() || text.contains("{{") || crate::color::Color::parse(text).is_some()
            });
            (!valid).then(|| {
                (
                    format!("{value} is not a color"),
                    "use a CSS color: \"#f4f1ea\" or \"rgb(244, 241, 234)\"".to_string(),
                )
            })
        }
        "text" | "textarea" | "html" | "richtext" | "inline_richtext" | "liquid" | "url"
        | "image_picker" | "video" | "video_url" | "font_picker" | "collection" | "product"
        | "page" | "blog" | "article" | "link_list" | "color_scheme" | "color_background"
        | "text_alignment" => (!value.is_string()).then(|| {
            (
                format!("a {kind} setting is a string, not {value}"),
                "write the value between quotes".to_string(),
            )
        }),
        _ => None,
    }
}

/// Reports the `theme_settings` the theme does not have, and the values their setting does not
/// take. `schema` is the theme's `config/settings_schema.json`, `None` without a theme to check
/// against (a warning then says so, when the data has theme settings).
pub fn check(
    theme_settings: &IndexMap<String, Json>,
    origins: &IndexMap<String, Origin>,
    schema: Option<&Json>,
    diagnostics: &mut Diagnostics,
) {
    if theme_settings.is_empty() {
        return;
    }
    let origin_of = |id: &str| {
        origins
            .get(id)
            .cloned()
            .unwrap_or_else(|| Origin::new("(data)", format!("/theme_settings/{id}")))
    };
    let Some(schema) = schema else {
        let first = origin_of(theme_settings.keys().next().expect("not empty"));
        diagnostics
            .warning(
                "unchecked_theme_settings",
                &first.file,
                "/theme_settings",
                "the theme_settings are not checked: there is no theme with a config/settings_schema.json",
            )
            .hint("run validate in the theme directory, or give it with --theme, to check them");
        return;
    };
    let settings = settings(schema);
    for (id, value) in theme_settings {
        let origin = origin_of(id);
        match settings.get(id.as_str()) {
            None => {
                let diagnostic = diagnostics.error(
                    "unknown_setting",
                    &origin.file,
                    &origin.pointer,
                    format!("the theme has no setting \"{id}\" in config/settings_schema.json"),
                );
                if let Some(close) = closest_match(id, settings.keys().copied()) {
                    diagnostic.hint(format!("did you mean \"{close}\"?"));
                }
            }
            Some(setting) => {
                if let Some((message, hint)) = problem(setting, value) {
                    diagnostics
                        .error("invalid_setting", &origin.file, &origin.pointer, message)
                        .hint(hint);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn problems(settings: Json) -> Vec<String> {
        let schema = json!([
            {"name": "theme_info"},
            {"name": "Cart", "settings": [
                {"type": "select", "id": "cart_type", "options": [{"value": "page", "label": "Page"}, {"value": "drawer", "label": "Drawer"}]},
                {"type": "range", "id": "page_width", "min": 1000, "max": 1600, "step": 100},
                {"type": "checkbox", "id": "show_vendor"},
                {"type": "color", "id": "accent"},
                {"type": "text", "id": "label"},
            ]},
        ]);
        let settings: IndexMap<String, Json> = serde_json::from_value(settings).unwrap();
        let mut diagnostics = Diagnostics::new();
        check(&settings, &IndexMap::new(), Some(&schema), &mut diagnostics);
        diagnostics
            .items
            .iter()
            .map(|item| {
                format!(
                    "{} {}: {} ({})",
                    item.code,
                    item.path,
                    item.message,
                    item.hint.as_deref().unwrap_or_default()
                )
            })
            .collect()
    }

    #[test]
    fn an_id_or_a_value_the_theme_does_not_take_is_an_error() {
        assert_eq!(
            problems(json!({"cart_typ": "page", "cart_type": "bogus"})),
            [
                "unknown_setting /theme_settings/cart_typ: the theme has no setting \"cart_typ\" in config/settings_schema.json (did you mean \"cart_type\"?)",
                "invalid_setting /theme_settings/cart_type: \"bogus\" is not an option of the setting (the setting takes \"page\", \"drawer\")",
            ]
        );
        assert_eq!(
            problems(json!({"cart_type": "drawr"})),
            [
                "invalid_setting /theme_settings/cart_type: \"drawr\" is not an option of the setting (did you mean \"drawer\"?)"
            ]
        );
        assert_eq!(
            problems(
                json!({"page_width": 2000, "show_vendor": "yes", "accent": "blurple", "label": 3})
            )
            .len(),
            4
        );
        assert!(
            problems(json!({"cart_type": "drawer", "page_width": 1200, "show_vendor": true, "accent": "#fff", "label": "Hi", "page_width_": null}))
                .iter()
                .all(|problem| problem.starts_with("unknown_setting /theme_settings/page_width_"))
        );
    }

    #[test]
    fn without_a_theme_the_settings_are_not_checked_and_a_warning_says_so() {
        let mut diagnostics = Diagnostics::new();
        let settings: IndexMap<String, Json> =
            serde_json::from_value(json!({"cart_type": "x"})).unwrap();
        check(&settings, &IndexMap::new(), None, &mut diagnostics);
        assert_eq!(diagnostics.warning_count(), 1);
        assert_eq!(diagnostics.items[0].code, "unchecked_theme_settings");
        // Nothing to say without theme settings.
        let mut diagnostics = Diagnostics::new();
        check(&IndexMap::new(), &IndexMap::new(), None, &mut diagnostics);
        assert!(diagnostics.is_empty());
    }
}
