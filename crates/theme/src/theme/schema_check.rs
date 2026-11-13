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
        for message in setting_rules(item, kind) {
            problem(
                "invalid_schema",
                format!("{name}: {message}: Shopify refuses the file"),
                None,
            );
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
                } else if ((max - min) / step).floor() + 1.0 > MOST_STEPS {
                    problem(
                        "invalid_schema",
                        format!(
                            "{name} has {} steps from {min} to {max}: Shopify takes 101 at most. Make the `step` bigger",
                            ((max - min) / step).floor() + 1.0
                        ),
                        None,
                    );
                }
            }
            "color_scheme_group" => {
                for message in scheme_roles(item) {
                    problem("invalid_schema", format!("{name}: {message}"), None);
                }
            }
            "color_palette" => {
                for message in palette(default) {
                    problem("invalid_schema", format!("{name}: {message}"), None);
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

/// The most characters of a setting's `label` and `info` (a `t:` key is not counted).
const LONGEST_LABEL: usize = 70;
const LONGEST_INFO: usize = 500;

/// The settings that pick a resource, and take no `default`.
const NO_DEFAULT: [&str; 5] = ["article", "blog", "collection", "page", "product"];

/// The settings that pick a list of resources, with a `limit` of 1 to 50.
const LISTS: [&str; 4] = [
    "article_list",
    "collection_list",
    "metaobject_list",
    "product_list",
];

/// The top-level tags a `richtext` default may have.
const RICHTEXT_BLOCKS: [&str; 9] = ["p", "ul", "ol", "h1", "h2", "h3", "h4", "h5", "h6"];

/// The tags an `inline_richtext` default may not have: it is not wrapped in a paragraph.
const NOT_INLINE: [&str; 15] = [
    "p",
    "div",
    "ul",
    "ol",
    "li",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "br",
    "blockquote",
    "table",
    "section",
];

/// The tags of an HTML text, as (name, closing, self-closing), and whether text sits at the
/// top level, outside every tag.
fn html_tags(html: &str) -> (Vec<(String, bool, bool)>, Vec<usize>) {
    let mut tags = Vec::new();
    // The depth at which each piece of text is.
    let mut text_depths = Vec::new();
    let mut depth: usize = 0;
    let mut rest = html;
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            if !rest.trim().is_empty() {
                text_depths.push(depth);
            }
            break;
        };
        if !rest[..open].trim().is_empty() {
            text_depths.push(depth);
        }
        let Some(close) = rest[open..].find('>') else {
            text_depths.push(depth);
            break;
        };
        let inner = &rest[open + 1..open + close];
        let closing = inner.starts_with('/');
        let name: String = inner
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let self_closing = inner.ends_with('/') || matches!(name.as_str(), "br" | "img" | "hr");
        if !name.is_empty() {
            if closing {
                depth = depth.saturating_sub(1);
            }
            tags.push((name, closing, self_closing));
            if !closing && !self_closing {
                depth += 1;
            }
        }
        rest = &rest[open + close + 1..];
    }
    (tags, text_depths)
}

/// What Shopify refuses in a `richtext` default: a top-level node that is not a paragraph, a
/// list or a heading (`Hello`, `<div>`).
fn richtext_default(html: &str) -> Option<String> {
    let (tags, text_depths) = html_tags(html);
    if text_depths.contains(&0) {
        return Some("its default has text outside a `<p>` or a `<ul>`".to_string());
    }
    let mut depth: usize = 0;
    for (name, closing, self_closing) in tags {
        if closing {
            depth = depth.saturating_sub(1);
            continue;
        }
        if depth == 0 && !RICHTEXT_BLOCKS.contains(&name.as_str()) {
            return Some(format!(
                "its default has a top-level `<{name}>`: only `<p>` and `<ul>` are"
            ));
        }
        if !self_closing {
            depth += 1;
        }
    }
    None
}

/// Whether a `color` default is a color, or a dynamic source (`{{ settings.colors.primary }}`).
fn is_color(text: &str) -> bool {
    let text = text.trim();
    if text.starts_with("{{") && text.ends_with("}}") {
        return true;
    }
    if let Some(hex) = text.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    let lower = text.to_ascii_lowercase();
    ["rgb(", "rgba(", "hsl(", "hsla("]
        .iter()
        .any(|function| lower.starts_with(function))
        && lower.ends_with(')')
}

/// Whether a text is written by the theme, not a `t:` key the locale gives.
fn literal(text: &str) -> Option<&str> {
    (!text.starts_with("t:")).then_some(text)
}

/// The Liquid environment `visible_if` is parsed with.
fn liquid() -> &'static lsf_liquid::Environment {
    static ENVIRONMENT: std::sync::OnceLock<lsf_liquid::Environment> = std::sync::OnceLock::new();
    ENVIRONMENT.get_or_init(|| crate::render::environment_with(&[]))
}

/// What Shopify refuses in one setting of a schema, beyond its required keys and the type of
/// its `default`: each message says what is wrong.
fn setting_rules(item: &Json, kind: &str) -> Vec<String> {
    let mut out = Vec::new();
    let default = item.get("default").filter(|default| !default.is_null());
    let text_default = default.and_then(Json::as_str).and_then(literal);
    for (key, most) in [("label", LONGEST_LABEL), ("info", LONGEST_INFO)] {
        if let Some(text) = item.get(key).and_then(Json::as_str).and_then(literal)
            && text.chars().count() > most
        {
            out.push(format!(
                "its `{key}` is {} characters long, more than {most}",
                text.chars().count()
            ));
        }
    }
    if let Some(id) = item.get("id").and_then(Json::as_str)
        && id.contains(char::is_whitespace)
    {
        out.push(format!("its id \"{id}\" has a space"));
    }
    if let Some(condition) = item.get("visible_if").and_then(Json::as_str) {
        if !condition.trim_start().starts_with("{{") {
            out.push("its `visible_if` does not start with `{{`".to_string());
        } else if let Err(error) = lsf_liquid::Template::parse(liquid(), condition) {
            out.push(format!("its `visible_if` is not valid Liquid ({error})"));
        }
    }
    match kind {
        "header" | "paragraph" if item.get("label").is_some() => {
            out.push(format!("a {kind} takes no `label`, only `content`"));
        }
        "text" | "textarea" => {
            for key in ["default", "placeholder"] {
                if let Some(value) = item
                    .get(key)
                    .filter(|value| !value.is_string() && !value.is_null())
                {
                    out.push(format!("its `{key}` {value} is not a string"));
                }
            }
        }
        _ if NO_DEFAULT.contains(&kind) && item.get("default").is_some() => {
            out.push(format!("a {kind} setting takes no `default`"));
        }
        _ if LISTS.contains(&kind) => {
            if let Some(limit) = item.get("limit")
                && limit
                    .as_i64()
                    .is_none_or(|limit| !(1..=50).contains(&limit))
            {
                out.push(format!("its `limit` {limit} is not from 1 to 50"));
            }
        }
        "richtext" => {
            if let Some(message) = text_default.and_then(richtext_default) {
                out.push(message);
            }
        }
        "inline_richtext" => {
            if let Some(html) = text_default {
                let (tags, _) = html_tags(html);
                if let Some((name, _, _)) = tags
                    .iter()
                    .find(|(name, _, _)| NOT_INLINE.contains(&name.as_str()))
                {
                    out.push(format!(
                        "its default has a `<{name}>`, which inline rich text does not take"
                    ));
                }
            }
        }
        "url" => {
            if let Some(url) = text_default
                && !matches!(url, "/collections" | "/collections/all")
            {
                out.push(format!(
                    "its default \"{url}\" is not `/collections` or `/collections/all`, the defaults a url takes"
                ));
            }
        }
        "color" => {
            if let Some(color) = text_default
                && !color.is_empty()
                && !is_color(color)
            {
                out.push(format!(
                    "its default \"{color}\" is not a color (`#121212`, `rgba(0, 0, 0, 0.5)`) or a dynamic source"
                ));
            }
        }
        "text_alignment" => {
            if let Some(alignment) = default
                && !matches!(alignment.as_str(), Some("left" | "center" | "right"))
            {
                out.push(format!(
                    "its default {alignment} is not left, center or right"
                ));
            }
        }
        "video_url" => {
            let accept = item.get("accept").and_then(Json::as_array);
            if let Some(provider) = accept
                .into_iter()
                .flatten()
                .find(|provider| !matches!(provider.as_str(), Some("youtube" | "vimeo")))
            {
                out.push(format!(
                    "its `accept` has {provider}: it takes only \"youtube\" and \"vimeo\""
                ));
            }
        }
        "select" | "radio" => {
            let mut values: Vec<&str> = Vec::new();
            for option in item
                .get("options")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(value) = option.get("value").and_then(Json::as_str) {
                    if values.contains(&value) {
                        out.push(format!("two of its options have the value \"{value}\""));
                    }
                    values.push(value);
                }
            }
        }
        _ => {}
    }
    out
}

/// The roles of a color scheme preview, each a definition of the scheme; those that may be a
/// gradient too (`{"solid": …, "gradient": …}`) are marked.
const SCHEME_ROLES: [(&str, bool); 10] = [
    ("background", true),
    ("text", false),
    ("primary_button", true),
    ("on_primary_button", false),
    ("primary_button_border", false),
    ("secondary_button", true),
    ("on_secondary_button", false),
    ("secondary_button_border", false),
    ("links", false),
    ("icons", false),
];

/// What Shopify refuses in the `role` of a `color_scheme_group`: a role it requires that is
/// missing, a role that names no id of the `definition`.
fn scheme_roles(item: &Json) -> Vec<String> {
    let Some(role) = item.get("role").and_then(Json::as_object) else {
        return Vec::new();
    };
    let ids: Vec<&str> = item
        .get("definition")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|definition| definition.get("id").and_then(Json::as_str))
        .collect();
    let mut out = Vec::new();
    for (key, gradient) in SCHEME_ROLES {
        let Some(value) = role.get(key) else {
            out.push(format!("its `role` has no `{key}`, which Shopify requires"));
            continue;
        };
        let named: Vec<&Json> = match value.as_object() {
            Some(object) if gradient => object.values().collect(),
            _ => vec![value],
        };
        for id in named {
            if id.as_str().is_none_or(|id| !ids.contains(&id)) {
                out.push(format!(
                    "`role.{key}` is {id}, which is not the id of a definition of the scheme"
                ));
            }
        }
    }
    out
}

/// The most colors a `color_palette` holds, and the fewest.
const PALETTE_COLORS: std::ops::RangeInclusive<usize> = 2..=20;

/// What Shopify refuses in the `default` of a `color_palette`: fewer than 2 colors or more than
/// 20, a key that is not a name, a color that is not hex without alpha.
fn palette(default: Option<&Json>) -> Vec<String> {
    let Some(colors) = default.and_then(Json::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if !PALETTE_COLORS.contains(&colors.len()) {
        out.push(format!(
            "its `default` has {} colors: a palette has from 2 to 20",
            colors.len()
        ));
    }
    for (key, color) in colors {
        let name = key.starts_with(|c: char| c.is_ascii_alphabetic())
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !name {
            out.push(format!(
                "the color \"{key}\" is not named with a letter, then letters, digits or `_`"
            ));
        }
        let hex = color.as_str().and_then(|color| color.strip_prefix('#'));
        let valid = hex.is_some_and(|hex| {
            matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit())
        });
        if !valid {
            out.push(format!(
                "the color \"{key}\" is {color}: a palette color is hex without alpha (`#FFF`, `#FF4416`)"
            ));
        }
    }
    out
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

/// A theme JSON file, parsed as Shopify reads it, with the lines of its values. `None` when it
/// does not parse: `lsf check` reports that as `invalid_json`.
fn parse_file(source: &str) -> Option<Parsed> {
    let text = lsf_core::json::theme_json(source).0;
    let json = serde_json::from_str::<Json>(&text).ok()?;
    Some(Parsed {
        lines: lsf_core::json::value_lines(&text),
        json,
        first_line: 1,
    })
}

/// The attributes of the `theme_info` of `config/settings_schema.json` that Shopify requires.
const THEME_INFO: [&str; 4] = [
    "theme_name",
    "theme_author",
    "theme_version",
    "theme_documentation_url",
];

/// The most color schemes a theme has.
const MOST_COLOR_SCHEMES: usize = 21;

/// What Shopify refuses on upload in `config/settings_schema.json`, whose source is `source`:
/// a file that is not a list, a group without a `name`, a `theme_info` without the attributes
/// it requires, the settings a section schema refuses, two settings with one id in the whole
/// file, more color schemes in `settings_data` (its `current` and its presets) than Shopify
/// takes. A refused file keeps its previous version on the store.
pub fn check_settings_schema(source: &str, data: Option<&Json>) -> Vec<SchemaProblem> {
    let Some(parsed) = parse_file(source) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let problem = |pointer: &str, code: &'static str, message: String| SchemaProblem {
        code,
        line: parsed.line(pointer),
        message,
        hint: None,
    };
    let Some(groups) = parsed.json.as_array() else {
        out.push(problem(
            "",
            "invalid_schema",
            "the file is not a list of setting groups: Shopify refuses it (\"Schema must be an array\")".to_string(),
        ));
        return out;
    };
    let mut ids: Vec<(&str, usize)> = Vec::new();
    let mut scheme_groups: Vec<(&str, String)> = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let at = format!("/{index}");
        let Some(name) = group.get("name").and_then(Json::as_str) else {
            out.push(problem(
                &at,
                "invalid_schema",
                format!(
                    "group {} has no `name`: Shopify refuses the file",
                    index + 1
                ),
            ));
            continue;
        };
        if name == "theme_info" {
            for key in THEME_INFO {
                match group.get(key) {
                    Some(Json::String(_)) => {}
                    Some(_) => out.push(problem(
                        &format!("{at}/{key}"),
                        "invalid_schema",
                        format!(
                            "the `{key}` of `theme_info` is not a string: Shopify refuses the file"
                        ),
                    )),
                    None => out.push(problem(
                        &at,
                        "invalid_schema",
                        format!("`theme_info` has no `{key}`: Shopify refuses the file"),
                    )),
                }
            }
            match (
                group.get("theme_support_email").is_some(),
                group.get("theme_support_url").is_some(),
            ) {
                (true, true) => out.push(problem(
                    &format!("{at}/theme_support_url"),
                    "invalid_schema",
                    "`theme_info` has both `theme_support_email` and `theme_support_url`: Shopify takes one of them".to_string(),
                )),
                (false, false) => out.push(problem(
                    &at,
                    "invalid_schema",
                    "`theme_info` has neither `theme_support_email` nor `theme_support_url`: Shopify refuses the file".to_string(),
                )),
                _ => {}
            }
            continue;
        }
        let Some(settings) = group.get("settings") else {
            continue;
        };
        check_settings(&parsed, settings, &format!("{at}/settings"), &mut out);
        for (position, setting) in settings.as_array().into_iter().flatten().enumerate() {
            let Some(id) = setting.get("id").and_then(Json::as_str) else {
                continue;
            };
            // Two in one group are reported with the group's settings.
            if ids
                .iter()
                .any(|(seen, group)| *seen == id && *group != index)
            {
                out.push(problem(
                    &format!("{at}/settings/{position}"),
                    "duplicate_setting_id",
                    format!(
                        "two settings have the id \"{id}\": Shopify refuses the file, and `settings.{id}` is the value of only one"
                    ),
                ));
            }
            ids.push((id, index));
            if setting.get("type").and_then(Json::as_str) == Some("color_scheme_group") {
                scheme_groups.push((id, format!("{at}/settings/{position}")));
            }
        }
    }
    // The color schemes are in `settings_data.json`: in `current`, and in each preset.
    if let Some(data) = data {
        let mut stored: Vec<&Json> = data.get("current").into_iter().collect();
        stored.extend(
            data.get("presets")
                .and_then(Json::as_object)
                .into_iter()
                .flat_map(|presets| presets.values()),
        );
        for (id, pointer) in &scheme_groups {
            let most = stored
                .iter()
                .filter_map(|values| values.get(*id).and_then(Json::as_object))
                .map(serde_json::Map::len)
                .max()
                .unwrap_or(0);
            if most > MOST_COLOR_SCHEMES {
                out.push(problem(
                    pointer,
                    "invalid_schema",
                    format!(
                        "config/settings_data.json has {most} color schemes in `{id}`: Shopify refuses more than {MOST_COLOR_SCHEMES}"
                    ),
                ));
            }
        }
    }
    out.sort_by_key(|problem| problem.line);
    out
}

/// What Shopify refuses on upload in `config/settings_data.json`: a `current` that names a
/// preset the file does not have.
pub fn check_settings_data(source: &str) -> Vec<SchemaProblem> {
    let Some(parsed) = parse_file(source) else {
        return Vec::new();
    };
    let Some(current) = parsed.json.get("current").and_then(Json::as_str) else {
        return Vec::new();
    };
    let presets: Vec<&String> = parsed
        .json
        .get("presets")
        .and_then(Json::as_object)
        .map(|presets| presets.keys().collect())
        .unwrap_or_default();
    if presets.iter().any(|preset| *preset == current) {
        return Vec::new();
    }
    vec![SchemaProblem {
        code: "invalid_settings_data",
        line: parsed.line("/current"),
        message: format!(
            "`current` is \"{current}\", which is not a preset ({}): Shopify refuses the file",
            presets
                .iter()
                .map(|preset| format!("\"{preset}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        hint: lsf_core::util::closest_match(current, presets.iter().map(|preset| preset.as_str()))
            .map(|close| format!("did you mean \"{close}\"?")),
    }]
}

/// Whether a schema is a section's or a theme block's: they take different attributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaOf {
    Section,
    Block,
}

/// The attributes of a section schema, as Shopify's section schema reference lists them, and
/// `templates`, which `enabled_on` and `disabled_on` replace.
const SECTION_ATTRIBUTES: &[&str] = &[
    "name",
    "tag",
    "class",
    "limit",
    "settings",
    "blocks",
    "max_blocks",
    "presets",
    "default",
    "locales",
    "enabled_on",
    "disabled_on",
    "templates",
];

/// The attributes of a theme block schema: "can only use the attributes listed".
const BLOCK_ATTRIBUTES: &[&str] = &["name", "settings", "blocks", "presets", "tag", "class"];

/// The tags a section can be wrapped in.
const SECTION_TAGS: &[&str] = &["article", "aside", "div", "footer", "header", "section"];

/// The page types of `enabled_on` and `disabled_on` templates (`request.page_type`).
const PAGE_TYPES: &[&str] = &[
    "*",
    "404",
    "article",
    "blog",
    "captcha",
    "cart",
    "collection",
    "list-collections",
    "customers/account",
    "customers/activate_account",
    "customers/addresses",
    "customers/login",
    "customers/order",
    "customers/register",
    "customers/reset_password",
    "gift_card",
    "index",
    "metaobject",
    "page",
    "password",
    "policy",
    "product",
    "search",
];

/// The most characters of a name Shopify takes: `t:` keys are not counted.
const LONGEST_NAME: usize = 25;

/// What Shopify refuses on upload in the attributes of a schema, other than its settings: a
/// `name` of more than 25 characters, an attribute the schema does not take, how its `blocks`
/// are declared, its `presets` and `default`, `max_blocks`, `limit`, `tag`, `class`,
/// `enabled_on` and `disabled_on`. Nothing for a schema that does not parse: [`check`] says so.
pub fn check_attributes(source: &str, of: SchemaOf) -> Vec<SchemaProblem> {
    let Some(Ok(parsed)) = parse(source) else {
        return Vec::new();
    };
    let Some(schema) = parsed.json.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut refuse = |pointer: &str, message: String| {
        out.push(SchemaProblem {
            code: "invalid_schema",
            line: parsed.line(pointer),
            message: format!("{message}: Shopify refuses the file"),
            hint: None,
        })
    };
    let (what, attributes) = match of {
        SchemaOf::Section => ("a section", SECTION_ATTRIBUTES),
        SchemaOf::Block => ("a theme block", BLOCK_ATTRIBUTES),
    };
    for key in schema.keys() {
        if !attributes.contains(&key.as_str()) {
            refuse(
                &format!("/{key}"),
                format!("`{key}` is not an attribute of the schema of {what}"),
            );
        }
    }
    if let Some(name) = schema.get("name").and_then(Json::as_str)
        && !name.starts_with("t:")
        && name.chars().count() > LONGEST_NAME
    {
        refuse(
            "/name",
            format!(
                "the name \"{name}\" is {} characters long, more than {LONGEST_NAME}",
                name.chars().count()
            ),
        );
    }
    let entries = schema
        .get("blocks")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    let local: Vec<(usize, &Json)> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| is_local(entry) && !is_shopify_type(entry))
        .collect();
    if of == SchemaOf::Block {
        for (index, entry) in &local {
            refuse(
                &format!("/blocks/{index}"),
                format!(
                    "the block \"{}\" is defined in the schema: a theme block takes no local blocks, only theme blocks and `@theme`",
                    entry.get("name").and_then(Json::as_str).unwrap_or_default()
                ),
            );
        }
    }
    let theme_blocks = entries
        .iter()
        .position(|entry| entry.get("type").and_then(Json::as_str) == Some("@theme"));
    if of == SchemaOf::Section
        && let Some(index) = theme_blocks
        && !local.is_empty()
    {
        refuse(
            &format!("/blocks/{index}"),
            "`@theme` and blocks defined in the schema (with a `name`) can not be used together"
                .to_string(),
        );
    }
    let mut types: Vec<&str> = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    for (index, entry) in &local {
        if let Some(kind) = entry.get("type").and_then(Json::as_str) {
            if types.contains(&kind) {
                refuse(
                    &format!("/blocks/{index}/type"),
                    format!("two blocks of the schema are of type \"{kind}\""),
                );
            }
            types.push(kind);
        }
        if let Some(name) = entry.get("name").and_then(Json::as_str) {
            if names.contains(&name) {
                refuse(
                    &format!("/blocks/{index}/name"),
                    format!("two blocks of the schema are named \"{name}\""),
                );
            }
            names.push(name);
        }
    }
    for (index, entry) in entries.iter().enumerate() {
        if is_shopify_type(entry) && entry.get("settings").is_some() {
            refuse(
                &format!("/blocks/{index}/settings"),
                format!(
                    "the block `{}` has `settings`: `@app` and `@theme` take only a `type`",
                    entry.get("type").and_then(Json::as_str).unwrap_or_default()
                ),
            );
        }
    }
    if let Some(max) = schema.get("max_blocks") {
        match max.as_i64() {
            Some(max) if max > 0 => {}
            Some(_) => refuse("/max_blocks", "`max_blocks` is not above 0".to_string()),
            None => refuse(
                "/max_blocks",
                "`max_blocks` is not a whole number".to_string(),
            ),
        }
    }
    if let Some(limit) = schema.get("limit")
        && limit.as_f64().is_some_and(|limit| limit >= 3.0)
    {
        refuse("/limit", format!("the `limit` {limit} is not 1 or 2"));
    }
    if let Some(tag) = schema.get("tag").and_then(Json::as_str) {
        match of {
            SchemaOf::Section if !SECTION_TAGS.contains(&tag) => refuse(
                "/tag",
                format!(
                    "the `tag` \"{tag}\" is not one a section takes ({})",
                    SECTION_TAGS.join(", ")
                ),
            ),
            SchemaOf::Block if tag.chars().count() > 50 => {
                refuse("/tag", "the `tag` is longer than 50 characters".to_string())
            }
            _ => {}
        }
    }
    if let Some(class) = schema.get("class").and_then(Json::as_str)
        && class.contains(['"', '<'])
    {
        refuse(
            "/class",
            format!("the `class` \"{class}\" has a character an HTML class attribute can not hold"),
        );
    }
    if of == SchemaOf::Section {
        if schema.contains_key("enabled_on") && schema.contains_key("disabled_on") {
            refuse(
                "/disabled_on",
                "`enabled_on` and `disabled_on` can not be used together".to_string(),
            );
        }
        for key in ["enabled_on", "disabled_on"] {
            let Some(value) = schema.get(key) else {
                continue;
            };
            let templates = value.get("templates").and_then(Json::as_array);
            let groups = value.get("groups").and_then(Json::as_array);
            if templates.is_none_or(Vec::is_empty) && groups.is_none_or(Vec::is_empty) {
                refuse(
                    &format!("/{key}"),
                    format!("`{key}` has neither `templates` nor `groups`"),
                );
            }
            for (index, template) in templates.into_iter().flatten().enumerate() {
                if template
                    .as_str()
                    .is_none_or(|name| !PAGE_TYPES.contains(&name))
                {
                    refuse(
                        &format!("/{key}/templates/{index}"),
                        format!("{template} in `{key}.templates` is not a page type"),
                    );
                }
            }
            for (index, group) in groups.into_iter().flatten().enumerate() {
                let known = group.as_str().is_some_and(|name| {
                    matches!(name, "*" | "header" | "footer" | "aside")
                        || name
                            .strip_prefix("custom.")
                            .is_some_and(|rest| !rest.is_empty())
                });
                if !known {
                    refuse(
                        &format!("/{key}/groups/{index}"),
                        format!(
                            "{group} in `{key}.groups` is not header, footer, aside or custom.<name>"
                        ),
                    );
                }
            }
        }
    }
    // The presets and the `default`: a name, blocks the schema takes, settings it has.
    let presets = schema.get("presets").and_then(Json::as_array);
    for (index, preset) in presets.into_iter().flatten().enumerate() {
        let at = format!("/presets/{index}");
        if preset.get("name").and_then(Json::as_str).is_none() {
            refuse(&at, format!("preset {} has no `name`", index + 1));
        }
        if of == SchemaOf::Section {
            preset_contents(schema, preset, &at, &types, &mut refuse);
        }
    }
    if of == SchemaOf::Section
        && let Some(default) = schema.get("default")
    {
        preset_contents(schema, default, "/default", &types, &mut refuse);
    }
    out.sort_by_key(|problem| problem.line);
    out
}

/// What Shopify refuses in a preset (or the `default`) of a section: a block of a type a schema
/// with local blocks does not define, settings when the schema has none.
fn preset_contents(
    schema: &serde_json::Map<String, Json>,
    preset: &Json,
    at: &str,
    local_types: &[&str],
    refuse: &mut impl FnMut(&str, String),
) {
    if !local_types.is_empty() {
        let mut named = Vec::new();
        preset_block_types(preset, at, &mut named);
        for (kind, pointer) in named {
            if !local_types.contains(&kind.as_str())
                && !kind.starts_with('@')
                && !kind.contains("://")
            {
                refuse(
                    &pointer,
                    format!(
                        "a block of a preset is of type \"{kind}\", which the schema does not define ({})",
                        local_types.join(", ")
                    ),
                );
            }
        }
    }
    let has_settings = schema
        .get("settings")
        .and_then(Json::as_array)
        .is_some_and(|settings| !settings.is_empty());
    if !has_settings
        && preset
            .get("settings")
            .and_then(Json::as_object)
            .is_some_and(|settings| !settings.is_empty())
    {
        refuse(
            &format!("{at}/settings"),
            "a preset sets settings, and the schema has none".to_string(),
        );
    }
}

/// Whether a block entry is `@app` or `@theme`.
fn is_shopify_type(entry: &Json) -> bool {
    matches!(
        entry.get("type").and_then(Json::as_str),
        Some("@app" | "@theme")
    )
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

    /// The messages for a section with one setting.
    fn one_setting(setting: &str) -> Vec<String> {
        check(&format!(
            "{{% schema %}}{{\"name\": \"S\", \"settings\": [{setting}]}}{{% endschema %}}"
        ))
        .into_iter()
        .map(|problem| problem.message)
        .collect()
    }

    #[test]
    fn setting_rules_shopify_enforces_on_upload_are_reported() {
        // #93: each setting is one Shopify refused on upload.
        let long_label = "L".repeat(300);
        let long_info = "I".repeat(1000);
        let cases: Vec<(String, &str)> = vec![
            (r#"{"type": "richtext", "id": "t", "label": "T", "default": "Hello"}"#.into(), "text outside a `<p>`"),
            (r#"{"type": "richtext", "id": "t", "label": "T", "default": "<div>Hello</div>"}"#.into(), "a top-level `<div>`"),
            (r#"{"type": "url", "id": "u", "label": "U", "default": "/pages/about"}"#.into(), "\"/pages/about\" is not `/collections`"),
            (r#"{"type": "url", "id": "u", "label": "U", "default": "https://example.com"}"#.into(), "is not `/collections`"),
            (r#"{"type": "color", "id": "c", "label": "C", "default": "red"}"#.into(), "\"red\" is not a color"),
            (r#"{"type": "color", "id": "c", "label": "C", "default": "notacolor"}"#.into(), "is not a color"),
            (r#"{"type": "inline_richtext", "id": "i", "label": "I", "default": "<p>Hello</p>"}"#.into(), "a `<p>`"),
            (r#"{"type": "inline_richtext", "id": "i", "label": "I", "default": "<h2>Hello</h2>"}"#.into(), "a `<h2>`"),
            (r#"{"type": "text_alignment", "id": "a", "label": "A", "default": "middle"}"#.into(), "\"middle\" is not left"),
            (r#"{"type": "text", "id": "t", "label": "T", "default": 5}"#.into(), "`default` 5 is not a string"),
            (r#"{"type": "text", "id": "t", "label": "T", "placeholder": 5}"#.into(), "`placeholder` 5 is not a string"),
            (r#"{"type": "product", "id": "p", "label": "P", "default": "x"}"#.into(), "takes no `default`"),
            (r#"{"type": "header", "content": "H", "id": "h", "label": "H"}"#.into(), "takes no `label`"),
            (format!(r#"{{"type": "text", "id": "t", "label": "{long_label}"}}"#), "`label` is 300 characters long"),
            (format!(r#"{{"type": "text", "id": "t", "label": "T", "info": "{long_info}"}}"#), "`info` is 1000 characters long"),
            (r#"{"type": "text", "id": "my id", "label": "T"}"#.into(), "has a space"),
            (r#"{"type": "select", "id": "s", "label": "S", "options": [{"value": "a", "label": "A"}, {"value": "a", "label": "B"}]}"#.into(), "two of its options have the value \"a\""),
            (r#"{"type": "range", "id": "r", "label": "R", "min": 0, "max": 101, "step": 1, "default": 0}"#.into(), "102 steps"),
            (r#"{"type": "collection_list", "id": "c", "label": "C", "limit": 51}"#.into(), "`limit` 51 is not from 1 to 50"),
            (r#"{"type": "product_list", "id": "p", "label": "P", "limit": 0}"#.into(), "`limit` 0 is not from 1 to 50"),
            (r#"{"type": "video_url", "id": "v", "label": "V", "accept": ["tiktok"]}"#.into(), "\"tiktok\""),
            (r#"{"type": "checkbox", "id": "b", "label": "B", "visible_if": "section.settings.x"}"#.into(), "does not start with `{{`"),
            (r#"{"type": "checkbox", "id": "b", "label": "B", "visible_if": "{{ section.settings.x"}"#.into(), "is not valid Liquid"),
        ];
        for (setting, expected) in &cases {
            let found = one_setting(setting);
            assert!(
                found.len() == 1 && found[0].contains(expected),
                "{setting}: {found:#?}"
            );
        }
    }

    #[test]
    fn settings_shopify_takes_are_not_reported() {
        for setting in [
            r#"{"type": "richtext", "id": "t", "label": "T", "default": "<p>Hello <strong>you</strong></p><ul><li>a</li></ul>"}"#,
            r#"{"type": "richtext", "id": "t", "label": "T", "default": "t:sections.rich_text.default"}"#,
            r#"{"type": "url", "id": "u", "label": "U", "default": "/collections/all"}"#,
            r##"{"type": "color", "id": "c", "label": "C", "default": "#00000026"}"##,
            r#"{"type": "color", "id": "c", "label": "C", "default": "{{ settings.color_palette.foreground }}"}"#,
            r#"{"type": "color", "id": "c", "label": "C", "default": "rgba(0, 0, 0, 0.5)"}"#,
            r#"{"type": "inline_richtext", "id": "i", "label": "I", "default": "<strong>Hi</strong> <a href=\"/\">there</a>"}"#,
            r#"{"type": "text_alignment", "id": "a", "label": "A", "default": "center"}"#,
            r#"{"type": "text", "id": "my-id", "label": "t:a.very.long.translation.key.that.goes.on.and.on.and.on.beyond.seventy.characters"}"#,
            r#"{"type": "range", "id": "r", "label": "R", "min": 0, "max": 100, "step": 1, "default": 0}"#,
            r#"{"type": "product_list", "id": "p", "label": "P", "limit": 50}"#,
            r#"{"type": "link_list", "id": "m", "label": "M", "default": "main-menu"}"#,
            r#"{"type": "video_url", "id": "v", "label": "V", "accept": ["youtube", "vimeo"]}"#,
            r#"{"type": "header", "content": "H", "info": "I", "visible_if": "{{ section.settings.a }}"}"#,
            r#"{"type": "checkbox", "id": "b", "label": "B", "visible_if": "{{ section.settings.x == 'a' and block.settings.y }}"}"#,
        ] {
            assert_eq!(one_setting(setting), Vec::<String>::new(), "{setting}");
        }
    }

    const THEME_INFO_OK: &str = r#"{ "name": "theme_info", "theme_name": "Probe", "theme_version": "1.0.0", "theme_author": "me", "theme_documentation_url": "https://example.com", "theme_support_url": "https://example.com" }"#;

    fn settings_schema(groups: &[&str], data: Option<Json>) -> Vec<(&'static str, usize, String)> {
        let source = format!("[\n{}\n]", groups.join(",\n"));
        check_settings_schema(&source, data.as_ref())
            .into_iter()
            .map(|problem| (problem.code, problem.line, problem.message))
            .collect()
    }

    #[test]
    fn a_settings_schema_shopify_refuses_is_reported() {
        // #183: an id taken in two groups, a group without a name.
        let found = settings_schema(
            &[
                THEME_INFO_OK,
                r#"{ "name": "One", "settings": [ { "type": "text", "id": "label", "label": "Label", "default": "one" } ] }"#,
                r#"{ "name": "Two", "settings": [ { "type": "text", "id": "label", "label": "Label again", "default": "two" } ] }"#,
                r#"{ "settings": [ { "type": "checkbox", "id": "flag", "label": "Flag", "default": true } ] }"#,
            ],
            None,
        );
        let summary: Vec<(&str, usize)> =
            found.iter().map(|(code, line, _)| (*code, *line)).collect();
        assert_eq!(
            summary,
            [("duplicate_setting_id", 4), ("invalid_schema", 5)],
            "{found:#?}"
        );
        // The other rules, one group each.
        let one = |group: &str| settings_schema(&[group], None);
        let cases = [
            (
                r#"{ "name": "theme_info", "theme_name": "P" }"#,
                "has no `theme_author`",
            ),
            (
                r#"{ "name": "theme_info", "theme_name": "P", "theme_version": 1, "theme_author": "a", "theme_documentation_url": "u", "theme_support_url": "u" }"#,
                "the `theme_version` of `theme_info` is not a string",
            ),
            (
                r#"{ "name": "theme_info", "theme_name": "P", "theme_version": "1", "theme_author": "a", "theme_documentation_url": "u", "theme_support_url": "u", "theme_support_email": "e" }"#,
                "has both `theme_support_email` and `theme_support_url`",
            ),
            (
                r#"{ "name": "G", "settings": [ { "type": "textbox", "id": "x", "label": "X" } ] }"#,
                "is of type \"textbox\"",
            ),
            (
                r#"{ "name": "G", "settings": [ { "type": "color_scheme_group", "id": "s", "definition": [] } ] }"#,
                "has no `role`",
            ),
            (
                r##"{ "name": "G", "settings": [ { "type": "color_palette", "id": "p", "default": { "a": "#000", "b": "#FFF", "c3": "#FF000080" } } ] }"##,
                "the color \"c3\" is \"#FF000080\"",
            ),
        ];
        for (group, expected) in cases {
            let found = one(group);
            assert!(
                found
                    .iter()
                    .any(|(_, _, message)| message.contains(expected)),
                "{group}: {found:#?}"
            );
        }
        // A palette of 21 colors.
        let colors: Vec<String> = (0..21).map(|n| format!("\"c{n}\": \"#000000\"")).collect();
        let palette = format!(
            r#"{{ "name": "G", "settings": [ {{ "type": "color_palette", "id": "p", "default": {{ {} }} }} ] }}"#,
            colors.join(", ")
        );
        assert!(
            one(&palette)
                .iter()
                .any(|(_, _, message)| message.contains("has 21 colors")),
            "{:#?}",
            one(&palette)
        );
        // A scheme group whose roles lack `icons`, or name what the definition does not have.
        let definition = r##"[ { "type": "color", "id": "bg", "label": "B", "default": "#FFF" }, { "type": "color", "id": "fg", "label": "F", "default": "#000" } ]"##;
        let roles = |icons: &str| {
            format!(
                r#"{{ "name": "G", "settings": [ {{ "type": "color_scheme_group", "id": "color_schemes", "definition": {definition}, "role": {{ "background": {{ "solid": "bg", "gradient": "bg" }}, "text": "nope", "primary_button": "fg", "on_primary_button": "bg", "primary_button_border": "fg", "secondary_button": "bg", "on_secondary_button": "fg", "secondary_button_border": "fg", "links": "fg"{icons} }} }} ] }}"#
            )
        };
        let messages: Vec<String> = one(&roles("")).into_iter().map(|found| found.2).collect();
        assert_eq!(messages.len(), 2, "{messages:#?}");
        assert!(messages[0].contains("has no `icons`") || messages[1].contains("has no `icons`"));
        assert!(
            messages
                .iter()
                .any(|message| message.contains("`role.text` is \"nope\""))
        );
        // 22 color schemes in settings_data.json.
        let schemes: serde_json::Map<String, Json> = (0..22)
            .map(|n| (format!("scheme-{n}"), serde_json::json!({"settings": {}})))
            .collect();
        let data = serde_json::json!({"current": {"color_schemes": schemes}, "presets": {}});
        let found = settings_schema(
            &[&roles(r#", "icons": "fg""#).replace("\"nope\"", "\"fg\"")],
            Some(data),
        );
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(found[0].2.contains("22 color schemes"), "{found:#?}");
        // An object, not a list.
        assert_eq!(
            check_settings_schema(r#"{ "name": "G" }"#, None)[0].code,
            "invalid_schema"
        );
    }

    #[test]
    fn a_settings_schema_shopify_takes_is_not_reported() {
        // No theme_info, a version with words, a support email.
        assert!(settings_schema(&[r#"{ "name": "G", "settings": [] }"#], None).is_empty());
        assert!(
            settings_schema(
                &[r#"{ "name": "theme_info", "theme_name": "P", "theme_version": "1.0.0-beta three", "theme_author": "a", "theme_documentation_url": "u", "theme_support_email": "e@x.com" }"#],
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn settings_data_whose_current_names_no_preset_is_refused() {
        let found = check_settings_data(
            "{\n  \"current\": \"Nope\",\n  \"presets\": { \"Default\": { \"label\": \"preset\" } }\n}",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "invalid_settings_data");
        assert_eq!(found[0].line, 2);
        assert!(
            found[0].message.contains("\"Nope\""),
            "{}",
            found[0].message
        );
        // A preset that is there, or values in `current`.
        assert!(
            check_settings_data(r#"{"current": "Default", "presets": {"Default": {}}}"#).is_empty()
        );
        assert!(
            check_settings_data(r#"{"current": {"label": 1, "unknown": [true]}, "presets": {}}"#)
                .is_empty()
        );
    }

    /// The messages of what Shopify refuses in the attributes of a schema, without the end.
    fn refused(of: SchemaOf, schema: &str) -> Vec<String> {
        check_attributes(&format!("{{% schema %}}{schema}{{% endschema %}}"), of)
            .into_iter()
            .map(|problem| {
                assert_eq!(problem.code, "invalid_schema");
                problem
                    .message
                    .trim_end_matches(": Shopify refuses the file")
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn schema_attributes_shopify_refuses_on_upload_are_reported() {
        use SchemaOf::{Block, Section};
        // #182: each defect is one Shopify refused on upload.
        let cases: &[(SchemaOf, &str, &str)] = &[
            (
                Section,
                r#"{"name": "Abcdefghijklmnopqrstuvwxyz"}"#,
                "is 26 characters long",
            ),
            (
                Section,
                r#"{"name": "M", "blocks": [{"type": "@theme"}, {"type": "local", "name": "Local"}]}"#,
                "`@theme` and blocks defined in the schema",
            ),
            (
                Section,
                r#"{"name": "M", "blocks": [{"type": "@app"}, {"type": "@theme"}, {"type": "a", "name": "A"}]}"#,
                "`@theme` and blocks defined in the schema",
            ),
            (
                Block,
                r#"{"name": "Item", "max_blocks": 3}"#,
                "`max_blocks` is not an attribute",
            ),
            (
                Section,
                r#"{"name": "D", "blocks": [{"type": "a", "name": "A"}, {"type": "a", "name": "B"}]}"#,
                "two blocks of the schema are of type \"a\"",
            ),
            (
                Section,
                r#"{"name": "D", "blocks": [{"type": "@app", "name": "App", "settings": []}]}"#,
                "the block `@app` has `settings`",
            ),
            (
                Section,
                r#"{"name": "D", "blocks": [{"type": "a", "name": "A"}], "default": {"blocks": [{"type": "zzz"}]}}"#,
                "a block of a preset is of type \"zzz\"",
            ),
            (
                Section,
                r#"{"name": "P", "presets": [{"category": "x"}]}"#,
                "preset 1 has no `name`",
            ),
            (
                Section,
                r#"{"name": "P", "blocks": [{"type": "a", "name": "A"}], "presets": [{"name": "P", "blocks": [{"type": "zzz"}]}]}"#,
                "a block of a preset is of type \"zzz\"",
            ),
            (
                Section,
                r#"{"name": "P", "presets": [{"name": "P", "settings": {"x": 1}}]}"#,
                "a preset sets settings, and the schema has none",
            ),
            (
                Section,
                r#"{"name": "M", "max_blocks": "5"}"#,
                "`max_blocks` is not a whole number",
            ),
            (
                Section,
                r#"{"name": "M", "max_blocks": 0}"#,
                "`max_blocks` is not above 0",
            ),
            (
                Section,
                r#"{"name": "L", "limit": 3}"#,
                "the `limit` 3 is not 1 or 2",
            ),
            (
                Section,
                r#"{"name": "T", "tag": "span"}"#,
                "the `tag` \"span\" is not one",
            ),
            (
                Section,
                r#"{"name": "C", "class": "a\" <b"}"#,
                "the `class` \"a\" <b\" has",
            ),
            (
                Section,
                r#"{"name": "F", "foo": 1}"#,
                "`foo` is not an attribute of the schema of a section",
            ),
            (
                Section,
                r#"{"name": "E", "enabled_on": {"templates": ["*"]}, "disabled_on": {"groups": ["footer"]}}"#,
                "`enabled_on` and `disabled_on` can not be used together",
            ),
            (
                Section,
                r#"{"name": "E", "enabled_on": {}}"#,
                "`enabled_on` has neither",
            ),
            (
                Section,
                r#"{"name": "E", "enabled_on": {"templates": ["nope"]}}"#,
                "\"nope\" in `enabled_on.templates` is not a page type",
            ),
            (
                Section,
                r#"{"name": "E", "enabled_on": {"groups": ["sidebar"]}}"#,
                "\"sidebar\" in `enabled_on.groups` is not header",
            ),
            (
                Block,
                r#"{"name": "E", "enabled_on": {"templates": ["*"]}}"#,
                "`enabled_on` is not an attribute of the schema of a theme block",
            ),
            (
                Block,
                r#"{"name": "B", "blocks": [{"type": "a", "name": "A"}]}"#,
                "the block \"A\" is defined in the schema: a theme block takes no local blocks",
            ),
            (
                Block,
                r#"{"name": "Abcdefghijklmnopqrstuvwxyz"}"#,
                "is 26 characters long",
            ),
        ];
        for (of, schema, expected) in cases {
            let found = refused(*of, schema);
            assert!(
                found.len() == 1 && found[0].contains(expected),
                "{schema}: {found:?}"
            );
        }
    }

    #[test]
    fn schema_attributes_shopify_takes_are_not_reported() {
        use SchemaOf::{Block, Section};
        // Shopify takes these: a `t:` name whose text is long, `max_blocks` above 50, presets
        // with more blocks than `max_blocks`, `@app` with local blocks, the legacy `templates`.
        for schema in [
            r#"{"name": "t:sections.a_very_long_translation_key.name"}"#,
            r#"{"name": "M", "max_blocks": 51, "blocks": [{"type": "a", "name": "A", "limit": 1}], "presets": [{"name": "P", "blocks": [{"type": "a"}, {"type": "a"}]}]}"#,
            r#"{"name": "A", "blocks": [{"type": "@app"}, {"type": "a", "name": "A"}], "settings": [{"type": "text", "id": "t", "label": "T"}], "presets": [{"name": "P", "settings": {"t": "x"}}]}"#,
            r#"{"name": "G", "tag": "section", "class": "a b", "limit": 1, "enabled_on": {"templates": ["product", "customers/account"], "groups": ["custom.overlay", "*"]}, "templates": ["index"], "locales": {}}"#,
            r#"{"name": "T", "blocks": [{"type": "@theme"}, {"type": "text"}], "presets": [{"name": "P", "blocks": [{"type": "text"}]}]}"#,
        ] {
            assert_eq!(refused(Section, schema), Vec::<String>::new(), "{schema}");
        }
        for schema in [
            r#"{"name": "B", "tag": "picture", "class": "x", "blocks": [{"type": "@theme"}], "presets": [{"name": "P"}], "settings": []}"#,
            r#"{"name": "B", "tag": null}"#,
        ] {
            assert_eq!(refused(Block, schema), Vec::<String>::new(), "{schema}");
        }
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
