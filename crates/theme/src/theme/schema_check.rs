//! What Shopify refuses in the `{% schema %}` of a section or a theme block when the theme is
//! uploaded: JSON that does not parse, a setting of a type it does not have, a setting without
//! its `id` or `label`, two settings with one id, a `select` whose `default` is not one of its
//! options, a `range` whose `default` is not one of its steps. [`check`] reads one file;
//! [`check_names`] adds what needs the rest of the theme: the theme blocks a schema names, and
//! the `t:` keys of the schema locale.
//!
//! The rules are those of Shopify's JSON schemas of a section and a theme block, which Theme
//! Check validates with.

use std::collections::HashMap;

use serde_json::Value as Json;

use super::schema;

/// A problem of a schema, at a line of its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaProblem {
    /// `invalid_schema`, `unknown_setting_type`, `duplicate_setting_id`, `missing_block` or
    /// `missing_schema_translation`.
    pub code: &'static str,
    /// The line in the file.
    pub line: usize,
    pub message: String,
    pub hint: Option<String>,
}

/// The types of setting a section or a block can have, as Shopify lists them.
pub const SETTING_TYPES: &[&str] = &[
    "article",
    "article_list",
    "blog",
    "checkbox",
    "collection",
    "collection_list",
    "color",
    "color_background",
    "color_palette",
    "color_scheme",
    "color_scheme_group",
    "font_picker",
    "header",
    "html",
    "image_picker",
    "inline_richtext",
    "link_list",
    "liquid",
    "metaobject",
    "metaobject_list",
    "number",
    "page",
    "paragraph",
    "product",
    "product_list",
    "radio",
    "range",
    "richtext",
    "select",
    "text",
    "text_alignment",
    "textarea",
    "url",
    "video",
    "video_url",
];

/// The setting type an unknown one probably is: one it starts (`image` for `image_picker`),
/// else the closest.
fn meant_type(kind: &str) -> Option<&'static str> {
    SETTING_TYPES
        .iter()
        .copied()
        .find(|known| known.starts_with(kind) && known[kind.len()..].starts_with('_'))
        .or_else(|| lsf_core::util::closest_match(kind, SETTING_TYPES.iter().copied()))
}

/// The most steps a `range` may have.
const MOST_STEPS: f64 = 101.0;

/// A parsed schema, and where its values are in the file.
struct Parsed {
    json: Json,
    /// The line of the file the schema's body starts on.
    first_line: usize,
    lines: HashMap<String, usize>,
}

impl Parsed {
    /// The line in the file of the value at `pointer`, or of the closest value above it.
    fn line(&self, pointer: &str) -> usize {
        let mut pointer = pointer;
        loop {
            if let Some(line) = self.lines.get(pointer) {
                return self.first_line + line - 1;
            }
            match pointer.rsplit_once('/') {
                Some((parent, _)) => pointer = parent,
                None => return self.first_line,
            }
        }
    }
}

/// The schema of a file: `None` without one (or an empty one), `Err` when it does not parse.
fn parse(source: &str) -> Option<Result<Parsed, SchemaProblem>> {
    let span = schema::block_span(source, "schema")?;
    let body = &source[span.clone()];
    if body.trim().is_empty() {
        return None;
    }
    let first_line = source[..span.start].matches('\n').count() + 1;
    let text = lsf_core::json::theme_json(body).0;
    Some(match serde_json::from_str::<Json>(&text) {
        Ok(json) => Ok(Parsed {
            lines: lsf_core::json::value_lines(&text),
            json,
            first_line,
        }),
        Err(error) => Err(SchemaProblem {
            code: "invalid_schema",
            line: first_line + error.line().max(1) - 1,
            message: format!(
                "the {{% schema %}} is not valid JSON ({}): Shopify refuses the file, and the settings get no default",
                // The line is the file's, not the schema's.
                error
                    .to_string()
                    .split(" at line ")
                    .next()
                    .unwrap_or_default()
            ),
            hint: None,
        }),
    })
}

/// How a message names a setting: `setting "gap"`, or `setting 3` without an id.
fn setting_name(item: &Json, index: usize) -> String {
    match item.get("id").and_then(Json::as_str) {
        Some(id) => format!("setting \"{id}\""),
        None => format!("setting {}", index + 1),
    }
}

fn number(value: Option<&Json>) -> Option<f64> {
    value.and_then(Json::as_f64)
}

/// Whether `value` is a whole number of `step`s, within rounding.
fn whole_steps(value: f64, step: f64) -> bool {
    let steps = value / step;
    (steps - steps.round()).abs() < 1e-6
}

/// The problems of a list of settings at `pointer` (`/settings`, `/blocks/0/settings`).
fn check_settings(parsed: &Parsed, list: &Json, pointer: &str, out: &mut Vec<SchemaProblem>) {
    let Some(items) = list.as_array() else {
        out.push(SchemaProblem {
            code: "invalid_schema",
            line: parsed.line(pointer),
            message: format!("`{}` of the schema is not a list", &pointer[1..]),
            hint: None,
        });
        return;
    };
    let mut seen: Vec<&str> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let at = format!("{pointer}/{index}");
        let line = parsed.line(&at);
        let name = setting_name(item, index);
        let mut problem = |code: &'static str, message: String, hint: Option<String>| {
            out.push(SchemaProblem {
                code,
                line,
                message,
                hint,
            })
        };
        if !item.is_object() {
            problem("invalid_schema", format!("{name} is not an object"), None);
            continue;
        }
        let Some(kind) = item.get("type").and_then(Json::as_str) else {
            problem("invalid_schema", format!("{name} has no `type`"), None);
            continue;
        };
        if !SETTING_TYPES.contains(&kind) {
            problem(
                "unknown_setting_type",
                format!(
                    "{name} is of type \"{kind}\", which Shopify does not have: Shopify refuses the file"
                ),
                meant_type(kind).map(|close| format!("did you mean \"{close}\"?")),
            );
            continue;
        }
        let required: &[&str] = match kind {
            "header" | "paragraph" => &["content"],
            "color_palette" => &["id", "default"],
            "color_scheme_group" => &["id", "definition", "role"],
            "font_picker" => &["id", "label", "default"],
            "range" => &["id", "label", "min", "max", "default"],
            "select" | "radio" => &["id", "label", "options"],
            "metaobject" | "metaobject_list" => &["id", "label", "metaobject_type"],
            "video_url" => &["id", "label", "accept"],
            _ => &["id", "label"],
        };
        let missing: Vec<String> = required
            .iter()
            .filter(|key| item.get(**key).is_none_or(Json::is_null))
            .map(|key| format!("`{key}`"))
            .collect();
        if !missing.is_empty() {
            problem(
                "invalid_schema",
                format!(
                    "{name} (a {kind}) has no {}: Shopify refuses the file",
                    missing.join(" and ")
                ),
                None,
            );
        }
        if let Some(id) = item.get("id").and_then(Json::as_str) {
            if seen.contains(&id) {
                problem(
                    "duplicate_setting_id",
                    format!(
                        "two settings have the id \"{id}\": Shopify refuses the file, and the second one is never read"
                    ),
                    None,
                );
            }
            seen.push(id);
        }
        let default = item.get("default");
        match kind {
            "select" | "radio" => {
                let Some(options) = item.get("options") else {
                    continue;
                };
                let Some(options) = options.as_array().filter(|options| !options.is_empty()) else {
                    problem(
                        "invalid_schema",
                        format!("the `options` of {name} are not a list of options"),
                        None,
                    );
                    continue;
                };
                let values: Vec<&str> = options
                    .iter()
                    .filter_map(|option| option.get("value").and_then(Json::as_str))
                    .collect();
                if values.len() < options.len() {
                    problem(
                        "invalid_schema",
                        format!("an option of {name} has no `value` string"),
                        None,
                    );
                }
                if let Some(default) = default.filter(|default| !default.is_null()) {
                    let text = default.as_str();
                    if text.is_none_or(|text| !values.contains(&text)) {
                        let shown = text.map_or_else(|| default.to_string(), str::to_string);
                        problem(
                            "invalid_schema",
                            format!(
                                "the default \"{shown}\" of {name} is not one of its options ({}): Shopify refuses the file",
                                values.join(", ")
                            ),
                            text.and_then(|text| {
                                lsf_core::util::closest_match(text, values.iter().copied())
                            })
                            .map(|close| format!("did you mean \"{close}\"?")),
                        );
                    }
                }
            }
            "range" => {
                let (Some(min), Some(max), Some(value)) = (
                    number(item.get("min")),
                    number(item.get("max")),
                    number(default),
                ) else {
                    for key in ["min", "max", "default", "step"] {
                        if item.get(key).is_some_and(|value| !value.is_number()) {
                            problem(
                                "invalid_schema",
                                format!("the `{key}` of {name} (a range) is not a number"),
                                None,
                            );
                        }
                    }
                    continue;
                };
                let step = match item.get("step") {
                    None => 1.0,
                    Some(step) => match step.as_f64() {
                        Some(step) if step > 0.0 => step,
                        _ => {
                            problem(
                                "invalid_schema",
                                format!("the `step` of {name} is not a number above 0"),
                                None,
                            );
                            continue;
                        }
                    },
                };
                if min >= max {
                    problem(
                        "invalid_schema",
                        format!("the `min` of {name} ({min}) is not below its `max` ({max})"),
                        None,
                    );
                } else if value < min || value > max {
                    problem(
                        "invalid_schema",
                        format!(
                            "the default {value} of {name} is not between its min {min} and max {max}: Shopify refuses the file"
                        ),
                        None,
                    );
                } else if !whole_steps(value - min, step) {
                    problem(
                        "invalid_schema",
                        format!(
                            "the default {value} of {name} is not a step of the range: from {min}, steps are {step} apart. Shopify refuses the file"
                        ),
                        None,
                    );
                } else if (max - min) / step > MOST_STEPS {
                    problem(
                        "invalid_schema",
                        format!(
                            "{name} has {} steps from {min} to {max}: Shopify takes 101 at most. Make the `step` bigger",
                            ((max - min) / step).floor()
                        ),
                        None,
                    );
                }
            }
            "checkbox" if default.is_some_and(|default| !default.is_boolean()) => problem(
                "invalid_schema",
                format!("the default of {name} (a checkbox) is not true or false"),
                None,
            ),
            "number" if default.is_some_and(|default| !default.is_number()) => problem(
                "invalid_schema",
                format!("the default of {name} (a number) is not a number"),
                None,
            ),
            _ => {}
        }
    }
}

/// Whether a block entry of a schema is a local block: one with a `name`, defined in the file
/// (`"type": "slide", "name": "Slide", "settings": [...]`).
fn is_local(entry: &Json) -> bool {
    entry.get("name").is_some()
}

/// What the `{% schema %}` of `source` has that strict JSON refuses and Shopify reads (a
/// trailing comma, an unknown escape such as `\“`), with its line in the file: warnings, since
/// Shopify takes the file and uses its values.
pub fn lenient(source: &str) -> Vec<SchemaProblem> {
    let Some(span) = schema::block_span(source, "schema") else {
        return Vec::new();
    };
    let first_line = source[..span.start].matches('\n').count() + 1;
    lsf_core::json::theme_json(&source[span])
        .1
        .into_iter()
        .map(|(line, leniency)| SchemaProblem {
            code: "lenient_json",
            line: first_line + line - 1,
            message: lenient_message("the {% schema %}", leniency),
            hint: None,
        })
        .collect()
}

/// The warning for what Shopify reads in a theme JSON text and strict JSON refuses.
pub fn lenient_message(what: &str, leniency: lsf_core::json::Leniency) -> String {
    format!(
        "{what} has {leniency}, which is not valid JSON: Shopify reads it, other JSON tools may not"
    )
}

/// What Shopify refuses in the `{% schema %}` of `source`, a section or a theme block, that the
/// file alone shows. Nothing for a file without a schema.
pub fn check(source: &str) -> Vec<SchemaProblem> {
    let parsed = match parse(source) {
        None => return Vec::new(),
        Some(Err(problem)) => return vec![problem],
        Some(Ok(parsed)) => parsed,
    };
    let mut out = Vec::new();
    if !parsed.json.is_object() {
        out.push(SchemaProblem {
            code: "invalid_schema",
            line: parsed.first_line,
            message: "the {% schema %} is not a JSON object".to_string(),
            hint: None,
        });
        return out;
    }
    if let Some(settings) = parsed.json.get("settings") {
        check_settings(&parsed, settings, "/settings", &mut out);
    }
    if let Some(blocks) = parsed.json.get("blocks").and_then(Json::as_array) {
        for (index, entry) in blocks.iter().enumerate() {
            let at = format!("/blocks/{index}");
            if entry.get("type").and_then(Json::as_str).is_none() {
                out.push(SchemaProblem {
                    code: "invalid_schema",
                    line: parsed.line(&at),
                    message: format!("block {} of the schema has no `type`", index + 1),
                    hint: None,
                });
                continue;
            }
            if let Some(settings) = entry.get("settings") {
                check_settings(&parsed, settings, &format!("{at}/settings"), &mut out);
            }
        }
    }
    out.sort_by_key(|problem| problem.line);
    out
}

/// What the rest of the theme tells about a schema.
pub struct ThemeNames<'a> {
    /// The theme blocks: the files of `blocks/`, without `.liquid`.
    pub blocks: &'a [String],
    /// The default schema locale (`locales/en.default.schema.json`), when the theme has one.
    pub schema_locale: Option<&'a Json>,
    /// Its file, for the messages.
    pub schema_locale_file: &'a str,
}

/// The block types of the presets (and the `default`) of a schema, with their pointers: each
/// `blocks` of them, a list or an object by id, and the blocks in those blocks.
fn preset_block_types(value: &Json, pointer: &str, out: &mut Vec<(String, String)>) {
    let entries: Vec<(String, &Json)> = match value.get("blocks") {
        Some(Json::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(index, item)| (format!("{pointer}/blocks/{index}"), item))
            .collect(),
        Some(Json::Object(items)) => items
            .iter()
            .map(|(key, item)| {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                (format!("{pointer}/blocks/{escaped}"), item)
            })
            .collect(),
        _ => Vec::new(),
    };
    for (at, entry) in entries {
        if let Some(kind) = entry.get("type").and_then(Json::as_str) {
            out.push((kind.to_string(), format!("{at}/type")));
        }
        preset_block_types(entry, &at, out);
    }
}

/// The `t:` keys of a schema's strings, with their pointers.
fn translation_keys(value: &Json, pointer: &str, out: &mut Vec<(String, String)>) {
    match value {
        Json::String(text) => {
            if let Some(key) = text.strip_prefix("t:")
                && !key.is_empty()
                && !key.contains(char::is_whitespace)
                && !key.contains("{{")
            {
                out.push((key.to_string(), pointer.to_string()));
            }
        }
        Json::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                translation_keys(item, &format!("{pointer}/{index}"), out);
            }
        }
        Json::Object(entries) => {
            for (key, item) in entries {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                translation_keys(item, &format!("{pointer}/{escaped}"), out);
            }
        }
        _ => {}
    }
}

/// Whether a theme block type is one the theme has, or one Shopify supplies.
fn is_known_block(kind: &str, theme: &ThemeNames<'_>) -> bool {
    kind.starts_with('@') || kind.contains("://") || theme.blocks.iter().any(|name| name == kind)
}

/// What the rest of the theme shows wrong in the schema of `source`: a theme block it names
/// that `blocks/` does not have (`missing_block`), a `t:` key the default schema locale does not
/// have (`missing_schema_translation`). Nothing when the schema does not parse: [`check`] says
/// so.
pub fn check_names(source: &str, theme: &ThemeNames<'_>) -> Vec<SchemaProblem> {
    let Some(Ok(parsed)) = parse(source) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let missing_block = |kind: &str, line: usize, what: &str| SchemaProblem {
        code: "missing_block",
        line,
        message: format!(
            "{what} \"{kind}\" is not a block of the theme: there is no blocks/{kind}.liquid"
        ),
        hint: lsf_core::util::closest_match(kind, theme.blocks.iter().map(String::as_str))
            .map(|close| format!("did you mean \"{close}\"?")),
    };
    let entries = parsed
        .json
        .get("blocks")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    // A schema with local blocks names its own types; one without names theme blocks.
    let local = entries.iter().any(is_local);
    for (index, entry) in entries.iter().enumerate() {
        let Some(kind) = entry.get("type").and_then(Json::as_str) else {
            continue;
        };
        if !is_local(entry) && !is_known_block(kind, theme) {
            out.push(missing_block(
                kind,
                parsed.line(&format!("/blocks/{index}/type")),
                "the block type",
            ));
        }
    }
    if !local {
        let mut named = Vec::new();
        if let Some(presets) = parsed.json.get("presets").and_then(Json::as_array) {
            for (index, preset) in presets.iter().enumerate() {
                preset_block_types(preset, &format!("/presets/{index}"), &mut named);
            }
        }
        if let Some(default) = parsed.json.get("default") {
            preset_block_types(default, "/default", &mut named);
        }
        for (kind, pointer) in named {
            if !is_known_block(&kind, theme) {
                out.push(missing_block(
                    &kind,
                    parsed.line(&pointer),
                    "a block of a preset is of type",
                ));
            }
        }
    }
    if let Some(locale) = theme.schema_locale {
        let mut keys = Vec::new();
        translation_keys(&parsed.json, "", &mut keys);
        let mut reported: Vec<String> = Vec::new();
        for (key, pointer) in keys {
            if reported.contains(&key) || has_string(locale, &key) {
                continue;
            }
            out.push(SchemaProblem {
                code: "missing_schema_translation",
                line: parsed.line(&pointer),
                message: format!(
                    "the key \"t:{key}\" of the schema is not in {}: the theme editor shows the key",
                    theme.schema_locale_file
                ),
                hint: Some(format!("add it to {}", theme.schema_locale_file)),
            });
            reported.push(key);
        }
    }
    out.sort_by_key(|problem| problem.line);
    out
}

/// Whether a locale has a key, as a string.
fn has_string(locale: &Json, key: &str) -> bool {
    let mut value = locale;
    for part in key.split('.') {
        match value.get(part) {
            Some(next) => value = next,
            None => return false,
        }
    }
    value.is_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(source: &str) -> Vec<(&'static str, usize, String)> {
        check(source)
            .into_iter()
            .map(|problem| (problem.code, problem.line, problem.message))
            .collect()
    }

    #[test]
    fn json_that_does_not_parse_is_reported_at_its_line() {
        let found = codes(
            "<p>\n{% schema %}\n{\n  \"name\": \"Hero\"\n  \"tag\": \"div\"\n}\n{% endschema %}",
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "invalid_schema");
        assert_eq!(found[0].1, 5);
        // A trailing comma and an unknown escape: Shopify reads the schema, so it is checked
        // as it reads it, and `lenient` warns.
        let source = "<p>\n{% schema %}\n{\n  \"name\": \"\\“Hero\",\n}\n{% endschema %}";
        assert!(codes(source).is_empty());
        let lenient: Vec<usize> = lenient(source).iter().map(|found| found.line).collect();
        assert_eq!(lenient, [4, 4]);
        // An empty schema, or none, is not checked.
        assert!(codes("{% schema %}{% endschema %}").is_empty());
        assert!(codes("<p>no schema</p>").is_empty());
    }

    #[test]
    fn settings_shopify_refuses_are_reported() {
        let source = r#"<div></div>
{% schema %}
{
  "name": "Hero",
  "settings": [
    { "type": "header", "content": "Layout" },
    { "type": "txt", "id": "heading", "label": "Heading" },
    { "type": "text", "id": "title" },
    { "type": "text", "id": "title", "label": "Again" },
    {
      "type": "select", "id": "align", "label": "Align",
      "options": [{ "value": "left", "label": "Left" }, { "value": "right", "label": "Right" }],
      "default": "center"
    },
    { "type": "range", "id": "gap", "label": "Gap", "min": 0, "max": 48, "step": 4, "default": 10 },
    { "type": "range", "id": "size", "label": "Size", "min": 10, "max": 0, "default": 5 },
    { "type": "range", "id": "far", "label": "Far", "min": 0, "max": 10, "default": 20 },
    { "type": "range", "id": "many", "label": "Many", "min": 0, "max": 500, "default": 0 },
    { "type": "range", "id": "ok", "label": "Fine", "min": 0, "max": 1, "step": 0.1, "default": 0.3 },
    { "type": "checkbox", "id": "on", "label": "On", "default": "yes" }
  ],
  "blocks": [
    { "type": "slide", "name": "Slide", "settings": [{ "type": "image", "id": "image", "label": "Image" }] }
  ]
}
{% endschema %}"#;
        let found = codes(source);
        let summary: Vec<(&str, usize)> =
            found.iter().map(|(code, line, _)| (*code, *line)).collect();
        assert_eq!(
            summary,
            [
                ("unknown_setting_type", 7),
                ("invalid_schema", 8),
                ("duplicate_setting_id", 9),
                ("invalid_schema", 10),
                ("invalid_schema", 15),
                ("invalid_schema", 16),
                ("invalid_schema", 17),
                ("invalid_schema", 18),
                ("invalid_schema", 20),
                ("unknown_setting_type", 23),
            ],
            "{found:#?}"
        );
        let problems = check(source);
        assert_eq!(problems[0].hint.as_deref(), Some("did you mean \"text\"?"));
        assert!(
            problems[1].message.contains("has no `label`"),
            "{}",
            problems[1].message
        );
        assert!(
            problems[3].message.contains("\"center\""),
            "{}",
            problems[3].message
        );
        assert!(
            problems[4].message.contains("not a step"),
            "{}",
            problems[4].message
        );
        assert_eq!(
            problems[9].hint.as_deref(),
            Some("did you mean \"image_picker\"?")
        );
    }

    #[test]
    fn theme_blocks_and_translations_are_checked_against_the_theme() {
        let source = r#"{% schema %}
{
  "name": "t:names.hero",
  "blocks": [{ "type": "@theme" }, { "type": "@app" }, { "type": "text" }, { "type": "buton" }],
  "settings": [{ "type": "text", "id": "a", "label": "t:settings.missing" }],
  "presets": [{ "name": "t:names.hero", "blocks": { "x": { "type": "prcie", "static": true } } }]
}
{% endschema %}"#;
        let blocks = vec![
            "text".to_string(),
            "button".to_string(),
            "price".to_string(),
        ];
        let locale = serde_json::json!({"names": {"hero": "Hero"}});
        let theme = ThemeNames {
            blocks: &blocks,
            schema_locale: Some(&locale),
            schema_locale_file: "locales/en.default.schema.json",
        };
        let found: Vec<(&str, usize, Option<String>)> = check_names(source, &theme)
            .into_iter()
            .map(|problem| (problem.code, problem.line, problem.hint))
            .collect();
        assert_eq!(
            found,
            [
                (
                    "missing_block",
                    4,
                    Some("did you mean \"button\"?".to_string())
                ),
                (
                    "missing_schema_translation",
                    5,
                    Some("add it to locales/en.default.schema.json".to_string())
                ),
                (
                    "missing_block",
                    6,
                    Some("did you mean \"price\"?".to_string())
                ),
            ]
        );
        // Local blocks name their own types, and their presets too.
        let local = r#"{% schema %}{"name": "x", "blocks": [{"type": "slide", "name": "Slide"}], "presets": [{"name": "x", "blocks": [{"type": "slide"}]}]}{% endschema %}"#;
        assert!(check_names(local, &theme).is_empty());
    }
}
