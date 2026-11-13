//! What Shopify refuses on upload in the values a JSON file stores for the settings of its
//! sections and blocks, checked against their schemas: a `range` value that is not a number or
//! is out of its `min` and `max`, Liquid in a text setting where Shopify takes only dynamic
//! sources (`{{ closest.product.title }}`), a `liquid` value over 50 kB. Shopify refuses the
//! template, the section group or `config/settings_data.json`.

use serde_json::Value as Json;

use super::Theme;
use super::liquid_settings::StoredSetting;

/// A value Shopify refuses.
#[derive(Clone, Debug)]
pub struct ValueProblem {
    pub setting: StoredSetting,
    pub message: String,
}

/// The settings whose value is text a merchant writes, where a `{{ }}` is a dynamic source.
const TEXT_SETTINGS: [&str; 4] = ["text", "textarea", "richtext", "inline_richtext"];

/// The most bytes of a `liquid` setting: "Content entered in these settings can't exceed 50kb".
const LIQUID_BYTES: usize = 50 * 1024;

/// Whether a text holds Liquid that is not a dynamic source: a tag (`{%`, `%}`), or a `{{` or a
/// `}}` that does not close or open an output.
fn has_liquid_syntax(text: &str) -> bool {
    if text.contains("{%") || text.contains("%}") {
        return true;
    }
    let mut rest = text;
    loop {
        let open = rest.find("{{");
        let close = rest.find("}}");
        match (open, close) {
            (None, None) => return false,
            // A `}}` before any `{{`, or a `{{` that never closes.
            (None, Some(_)) | (Some(_), None) => return true,
            (Some(open), Some(close)) if close < open => return true,
            (Some(open), Some(_)) => {
                let after = &rest[open + 2..];
                let Some(end) = after.find("}}") else {
                    return true;
                };
                // A `{{` inside the output does not close it.
                if after[..end].contains("{{") {
                    return true;
                }
                rest = &after[end + 2..];
            }
        }
    }
}

/// How a message names a number: `10`, not `10.0`.
fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// The problem of one stored value, if Shopify refuses it.
fn problem(setting: &StoredSetting) -> Option<String> {
    let id = &setting.def.id;
    match setting.def.kind.as_str() {
        "range" => {
            let value = match &setting.value {
                Json::Null => return None,
                Json::Number(value) => value.as_f64(),
                Json::String(text) => text.trim().parse::<f64>().ok(),
                _ => None,
            };
            let Some(value) = value.filter(|value| value.is_finite()) else {
                return Some(format!(
                    "Setting '{id}' must be a valid number: it is {}",
                    setting.value
                ));
            };
            let (min, max) = setting.def.range?;
            if value > max {
                Some(format!(
                    "Setting '{id}' can't be greater than {}: it is {}",
                    number(max),
                    number(value)
                ))
            } else if value < min {
                Some(format!(
                    "Setting '{id}' can't be less than {}: it is {}",
                    number(min),
                    number(value)
                ))
            } else {
                None
            }
        }
        kind if TEXT_SETTINGS.contains(&kind) => {
            let text = setting.value.as_str()?;
            has_liquid_syntax(text).then(|| {
                format!(
                    "Setting '{id}' can't include Liquid syntax ('{{{{', '}}}}', '{{%' or '%}}') without valid dynamic sources"
                )
            })
        }
        "liquid" => {
            let text = setting.value.as_str()?;
            (text.len() > LIQUID_BYTES).then(|| {
                format!(
                    "Setting '{id}' is invalid: Liquid file size cannot exceed 50 kilobytes (it has {} bytes)",
                    text.len()
                )
            })
        }
        _ => None,
    }
}

impl Theme {
    /// What Shopify refuses in the values a JSON file of the theme stores for the settings of
    /// its sections and blocks.
    pub fn setting_value_problems(&self, path: &str) -> Vec<ValueProblem> {
        self.stored_settings(path)
            .into_iter()
            .filter_map(|setting| {
                problem(&setting).map(|message| ValueProblem { setting, message })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::schema::SettingDef;

    fn of(kind: &str, value: Json) -> Option<String> {
        problem(&StoredSetting {
            path: vec!["settings".to_string(), "x".to_string()],
            def: SettingDef {
                kind: kind.to_string(),
                id: "x".to_string(),
                default: None,
                metaobject_type: None,
                first_option: None,
                range: (kind == "range").then_some((0.0, 10.0)),
            },
            value,
        })
    }

    #[test]
    fn values_shopify_refuses_on_upload() {
        use serde_json::json;
        // #186: out of the range, not a number, a dynamic source in a range, Liquid in text.
        assert_eq!(
            of("range", json!(99)).as_deref(),
            Some("Setting 'x' can't be greater than 10: it is 99")
        );
        assert_eq!(
            of("range", json!(-1)).as_deref(),
            Some("Setting 'x' can't be less than 0: it is -1")
        );
        for value in [
            json!("abc"),
            json!("{{ page.metafields.ns.n.value }}"),
            json!(true),
        ] {
            assert!(
                of("range", value.clone()).is_some_and(|m| m.contains("must be a valid number")),
                "{value}"
            );
        }
        for text in [
            "{% if true %}yes{% endif %}",
            "{{ page.title",
            "a }} b",
            "{{ a {{ b }}",
            "%}",
        ] {
            assert!(
                of("text", json!(text)).is_some_and(|m| m.contains("can't include Liquid syntax")),
                "{text}"
            );
        }
        assert!(of("richtext", json!("<p>{% render 'x' %}</p>")).is_some());
        let big = "a".repeat(60 * 1024);
        assert!(of("liquid", json!(big)).is_some_and(|m| m.contains("50 kilobytes")));
    }

    #[test]
    fn values_shopify_takes() {
        use serde_json::json;
        // In range, a number as a string, nothing; a dynamic source, plain text; Liquid in a
        // `liquid` setting.
        for value in [json!(5), json!(0), json!(10), json!("7"), json!(null)] {
            assert_eq!(of("range", value.clone()), None, "{value}");
        }
        for text in [
            "{{ closest.product.title }}",
            "Hi {{ product.title }} and {{ product.vendor }}",
            "50% off { not liquid }",
        ] {
            assert_eq!(of("text", json!(text)), None, "{text}");
        }
        assert_eq!(of("liquid", json!("{% if true %}yes{% endif %}")), None);
        assert_eq!(of("select", json!("{% x %}")), None);
    }
}
