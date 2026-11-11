//! What a theme names that it does not have: a snippet a `render` or an `include` loads, a
//! section of `{% section %}` or of a JSON template, a section group, a key of the `t` filter
//! that the default locale does not have. Each renders as an error on Shopify.

use lsf_theme::Theme;
use lsf_theme::theme::schema_check::{self, SchemaProblem, ThemeNames};
use serde_json::Value as Json;

/// A name of the theme that is missing.
#[derive(Debug, PartialEq)]
pub struct Missing {
    pub code: &'static str,
    pub line: usize,
    pub message: String,
    pub hint: Option<String>,
}

/// Tags whose body is not Liquid.
const RAW_TAGS: [&str; 6] = [
    "schema",
    "raw",
    "javascript",
    "stylesheet",
    "doc",
    "comment",
];

/// A statement of a Liquid source: an output (`name` empty) or a tag, with what follows its
/// name, and its line.
struct Statement<'a> {
    name: &'a str,
    markup: &'a str,
    line: usize,
}

/// The statements of a Liquid source, outside comments and the tags whose body is not Liquid.
/// The statements of `{% liquid %}` are listed one by one.
fn statements(source: &str) -> Vec<Statement<'_>> {
    let mut found = Vec::new();
    let mut position = 0;
    while let Some(start) = [source[position..].find("{{"), source[position..].find("{%")]
        .into_iter()
        .flatten()
        .min()
        .map(|start| position + start)
    {
        let output = source[start..].starts_with("{{");
        let Some(length) = source[start..].find(if output { "}}" } else { "%}" }) else {
            break;
        };
        let markup = source[start + 2..start + length].trim_matches('-').trim();
        let line = source[..start].matches('\n').count() + 1;
        position = start + length + 2;
        if output {
            found.push(Statement {
                name: "",
                markup,
                line,
            });
            continue;
        }
        if markup.starts_with('#') {
            continue;
        }
        let name = markup.split_whitespace().next().unwrap_or_default();
        if RAW_TAGS.contains(&name) {
            position = source[position..]
                .find(&format!("end{name}"))
                .map_or(source.len(), |index| position + index);
            continue;
        }
        let rest = &markup[name.len()..];
        if name != "liquid" {
            found.push(Statement {
                name,
                markup: rest.trim(),
                line,
            });
            continue;
        }
        let mut quiet: Option<String> = None;
        for (offset, statement) in rest.lines().enumerate() {
            let statement = statement.trim();
            let name = statement.split_whitespace().next().unwrap_or_default();
            if let Some(end) = &quiet {
                if name == end {
                    quiet = None;
                }
                continue;
            }
            if RAW_TAGS.contains(&name) {
                quiet = Some(format!("end{name}"));
                continue;
            }
            if name.is_empty() || statement.starts_with('#') {
                continue;
            }
            found.push(Statement {
                name,
                markup: statement[name.len()..].trim(),
                line: line + offset,
            });
        }
    }
    found
}

/// The string literal a markup starts with: `'card', product: p` → `card`.
fn leading_literal(markup: &str) -> Option<(&str, &str)> {
    let quote = markup.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let end = markup[1..].find(quote)? + 1;
    Some((&markup[1..end], &markup[end + 1..]))
}

/// Splits markup on the commas that are outside string literals.
fn top_level_parts(markup: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (index, c) in markup.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == ',' => {
                parts.push(&markup[start..index]);
                start = index + 1;
            }
            None => {}
        }
    }
    parts.push(&markup[start..]);
    parts
}

/// The key an expression translates: `'products.add' | t` → `products.add`. Only a literal
/// that is the input of the filter: `'a' | append: 'b' | t` translates no literal key.
fn translated_key(expression: &str) -> Option<&str> {
    let (key, rest) = leading_literal(expression.trim())?;
    let rest = rest.trim_start().strip_prefix('|')?.trim_start();
    let filter: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (filter == "t" || filter == "translate").then_some(key)
}

/// The keys of the `t` filter in a statement, with the expressions they are the input of.
fn translated_keys<'a>(statement: &Statement<'a>) -> Vec<&'a str> {
    let expressions: Vec<&str> = match statement.name {
        "" | "echo" => top_level_parts(statement.markup)
            .into_iter()
            .take(1)
            .collect(),
        "assign" => statement
            .markup
            .split_once('=')
            .map(|(_, value)| value)
            .into_iter()
            .collect(),
        // The arguments of a snippet: `text: 'key' | t`.
        "render" | "include" | "content_for" => top_level_parts(statement.markup)
            .into_iter()
            .skip(1)
            .filter_map(|part| {
                let (name, value) = part.split_once(':')?;
                let name = name.trim();
                (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
                    .then_some(value)
            })
            .collect(),
        _ => Vec::new(),
    };
    expressions.into_iter().filter_map(translated_key).collect()
}

/// Whether the default locale has a key: a string, or the object of a pluralized one.
fn has_key(locale: &Json, key: &str) -> bool {
    let mut value = locale;
    for part in key.split('.') {
        match value.get(part) {
            Some(next) => value = next,
            None => return false,
        }
    }
    value.is_string() || value.is_object()
}

/// The files of a directory, without their extension, for "did you mean" hints.
fn names(theme: &Theme, directory: &str, extension: &str) -> Vec<String> {
    theme
        .files()
        .list(directory)
        .iter()
        .filter_map(|file| file.strip_suffix(extension))
        .map(str::to_string)
        .collect()
}

fn did_you_mean(name: &str, candidates: &[String]) -> Option<String> {
    lsf_core::util::closest_match(name, candidates.iter().map(String::as_str))
        .map(|close| format!("did you mean \"{close}\"?"))
}

/// The theme's default locale, `None` when it has none to check against.
pub fn default_locale(theme: &Theme) -> Option<std::sync::Arc<Json>> {
    let default = theme.default_locale();
    theme
        .json(&format!("locales/{default}.default.json"))
        .ok()
        .flatten()
}

/// What the schemas of sections and theme blocks are checked against: the theme blocks, and
/// the default schema locale.
pub struct SchemaNames {
    blocks: Vec<String>,
    locale: Option<std::sync::Arc<Json>>,
    locale_file: String,
}

impl SchemaNames {
    pub fn of(theme: &Theme) -> SchemaNames {
        let locale_file = format!("locales/{}.default.schema.json", theme.default_locale());
        SchemaNames {
            blocks: names(theme, "blocks", ".liquid"),
            locale: theme.json(&locale_file).ok().flatten(),
            locale_file,
        }
    }

    /// What Shopify refuses in the `{% schema %}` of a section or a theme block.
    pub fn problems(&self, source: &str) -> Vec<SchemaProblem> {
        let mut problems = schema_check::check(source);
        problems.extend(schema_check::check_names(
            source,
            &ThemeNames {
                blocks: &self.blocks,
                schema_locale: self.locale.as_deref(),
                schema_locale_file: &self.locale_file,
            },
        ));
        problems.sort_by_key(|problem| problem.line);
        problems
    }
}

/// What a Liquid file names that the theme does not have.
pub fn in_liquid(theme: &Theme, source: &str, locale: Option<&Json>) -> Vec<Missing> {
    let snippets = names(theme, "snippets", ".liquid");
    let sections = names(theme, "sections", ".liquid");
    let groups = names(theme, "sections", ".json");
    let default = theme.default_locale();
    let mut missing = Vec::new();
    for statement in statements(source) {
        let literal = leading_literal(statement.markup).map(|(name, _)| name);
        match (statement.name, literal) {
            ("render" | "include", Some(name)) if !snippets.iter().any(|s| s == name) => {
                missing.push(Missing {
                    code: "missing_snippet",
                    line: statement.line,
                    message: format!(
                        "{{% {} '{name}' %}} loads snippets/{name}.liquid, which the theme does not have",
                        statement.name
                    ),
                    hint: did_you_mean(name, &snippets),
                })
            }
            ("section", Some(name)) if !sections.iter().any(|s| s == name) => {
                missing.push(Missing {
                    code: "missing_section",
                    line: statement.line,
                    message: format!(
                        "{{% section '{name}' %}} renders sections/{name}.liquid, which the theme does not have"
                    ),
                    hint: did_you_mean(name, &sections),
                })
            }
            ("sections", Some(name)) if !groups.iter().any(|s| s == name) => {
                missing.push(Missing {
                    code: "missing_section_group",
                    line: statement.line,
                    message: format!(
                        "{{% sections '{name}' %}} renders sections/{name}.json, which the theme does not have"
                    ),
                    hint: did_you_mean(name, &groups),
                })
            }
            ("content_for", Some("block")) => {
                let blocks = names(theme, "blocks", ".liquid");
                let kind = top_level_parts(statement.markup)
                    .into_iter()
                    .skip(1)
                    .filter_map(|part| part.split_once(':'))
                    .find(|(name, _)| name.trim() == "type")
                    .and_then(|(_, value)| leading_literal(value.trim()))
                    .map(|(kind, _)| kind);
                if let Some(kind) = kind
                    && !blocks.iter().any(|block| block == kind)
                {
                    missing.push(Missing {
                        code: "missing_block",
                        line: statement.line,
                        message: format!(
                            "{{% content_for 'block', type: '{kind}' %}} renders blocks/{kind}.liquid, which the theme does not have"
                        ),
                        hint: did_you_mean(kind, &blocks),
                    });
                }
            }
            _ => {}
        }
        let Some(locale) = locale else {
            continue;
        };
        for key in translated_keys(&statement) {
            // A key that Liquid builds, or that an app or Shopify gives.
            if key.is_empty() || key.contains("{{") || key.contains(' ') || has_key(locale, key) {
                continue;
            }
            missing.push(Missing {
                code: "missing_translation",
                line: statement.line,
                message: format!(
                    "the key \"{key}\" of the t filter is not in locales/{default}.default.json: the page shows \"translation missing\""
                ),
                hint: Some(format!("add it to locales/{default}.default.json")),
            });
        }
    }
    missing
}

/// The line of a JSON file where `"type"` names `kind`, at or after `from` (a line index).
fn type_line(source: &str, kind: &str, from: usize) -> Option<usize> {
    let needle = format!("\"{kind}\"");
    source
        .lines()
        .enumerate()
        .skip(from)
        .find(|(_, line)| line.contains("\"type\"") && line.contains(&needle))
        .map(|(index, _)| index + 1)
}

/// The block types of `blocks` (the blocks of a section, or of a block, in a JSON template) that
/// are neither a block the section defines (`local`) nor a theme block, with the blocks of
/// those blocks.
fn missing_blocks(
    blocks: Option<&Json>,
    local: &[String],
    theme_blocks: &[String],
    where_: &str,
    source: &str,
    from: usize,
    missing: &mut Vec<Missing>,
) {
    let Some(entries) = blocks.and_then(Json::as_object) else {
        return;
    };
    for (key, block) in entries {
        let Some(kind) = block.get("type").and_then(Json::as_str) else {
            continue;
        };
        // App blocks are checked by `--app`.
        let known = kind.contains("://")
            || kind.starts_with('@')
            || local.iter().any(|name| name == kind)
            || theme_blocks.iter().any(|name| name == kind);
        if !known {
            let mut candidates = local.to_vec();
            candidates.extend(theme_blocks.iter().cloned());
            missing.push(Missing {
                code: "missing_block",
                line: type_line(source, kind, from).unwrap_or(1),
                message: format!(
                    "the block \"{key}\" of {where_} is of type \"{kind}\": not a block of the theme (no blocks/{kind}.liquid){}",
                    if local.is_empty() {
                        ""
                    } else {
                        " nor of its section"
                    }
                ),
                hint: did_you_mean(kind, &candidates),
            });
        }
        // The blocks of a theme block are theme blocks.
        missing_blocks(
            block.get("blocks"),
            &[],
            theme_blocks,
            &format!("the block \"{key}\""),
            source,
            from,
            missing,
        );
    }
}

/// The string literals of a markup that are the input of `asset_url`: `'base.css' | asset_url`.
fn asset_literals(markup: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut quote: Option<(char, usize)> = None;
    for (index, c) in markup.char_indices() {
        match quote {
            Some((open, start)) if c == open => {
                quote = None;
                let rest = markup[index + 1..].trim_start();
                let Some(rest) = rest.strip_prefix('|') else {
                    continue;
                };
                let filter: String = rest
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if filter == "asset_url" {
                    found.push(&markup[start..index]);
                }
            }
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some((c, index + 1)),
            None => {}
        }
    }
    found
}

/// The files of `assets/` that `asset_url` names, as literals, and the theme does not have:
/// a warning each, since the page still renders (the URL leads to nothing).
pub fn missing_assets(theme: &Theme, source: &str) -> Vec<Missing> {
    let assets: Vec<String> = theme
        .files()
        .list("assets")
        .iter()
        .map(|name| name.strip_suffix(".liquid").unwrap_or(name).to_string())
        .collect();
    let mut missing = Vec::new();
    for statement in statements(source) {
        for name in asset_literals(statement.markup) {
            if name.is_empty()
                || name.contains("{{")
                || name.contains('/')
                || assets.iter().any(|asset| asset == name)
            {
                continue;
            }
            missing.push(Missing {
                code: "missing_asset",
                line: statement.line,
                message: format!(
                    "'{name}' | asset_url names assets/{name}, which the theme does not have: the URL leads to nothing"
                ),
                hint: did_you_mean(name, &assets),
            });
        }
    }
    missing
}

/// The section types of a JSON template or section group that the theme has no file for.
pub fn in_json_template(theme: &Theme, path: &str) -> Vec<Missing> {
    let Ok(Some(json)) = theme.json(path) else {
        return Vec::new();
    };
    let source = theme.files().read(path).unwrap_or_default();
    let sections = names(theme, "sections", ".liquid");
    let theme_blocks = names(theme, "blocks", ".liquid");
    let mut missing = Vec::new();
    let Some(entries) = json.get("sections").and_then(Json::as_object) else {
        return missing;
    };
    for (key, section) in entries {
        let Some(kind) = section.get("type").and_then(Json::as_str) else {
            continue;
        };
        // The sections of apps live with the app, not in the theme, and Shopify has its own
        // `apps` section for a theme without `sections/apps.liquid`.
        if kind.contains("://") || kind == "apps" {
            continue;
        }
        if sections.iter().any(|s| s == kind) {
            // The block types the section defines in its schema.
            let schema = theme.schema(&format!("sections/{kind}.liquid"));
            let local: Vec<String> = schema
                .raw
                .get("blocks")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter(|entry| entry.get("name").is_some())
                .filter_map(|entry| entry.get("type").and_then(Json::as_str))
                .map(str::to_string)
                .collect();
            // The blocks are after the section's type in the file.
            let from = type_line(&source, kind, 0).map_or(0, |line| line - 1);
            missing_blocks(
                section.get("blocks"),
                &local,
                &theme_blocks,
                &format!("the section \"{key}\""),
                &source,
                from,
                &mut missing,
            );
            continue;
        }
        let needle = format!("\"{kind}\"");
        let line = source
            .lines()
            .position(|line| line.contains("\"type\"") && line.contains(&needle))
            .map_or(1, |index| index + 1);
        missing.push(Missing {
            code: "missing_section",
            line,
            message: format!(
                "the section \"{key}\" is of type \"{kind}\", and the theme has no sections/{kind}.liquid"
            ),
            hint: did_you_mean(kind, &sections),
        });
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(source: &str) -> Vec<String> {
        statements(source)
            .iter()
            .flat_map(|statement| translated_keys(statement))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn finds_the_keys_the_t_filter_translates() {
        assert_eq!(
            keys("{{ 'a.b' | t }} {{- \"c.d\" | t: count: 2 -}} {% echo 'e' | translate %}"),
            ["a.b", "c.d", "e"]
        );
        assert_eq!(
            keys("{% assign x = 'f.g' | t %}{% render 'card', label: 'h.i' | t, n: 2 %}"),
            ["f.g", "h.i"]
        );
        // Not a literal key: what the filter gets is built.
        assert!(
            keys("{{ 'a.' | append: 'b' | t }}{{ x | t }}{{ 'a' | replace: 'a', 'b' | t }}")
                .is_empty()
        );
        // Comments and raw are not Liquid.
        assert!(keys("{% comment %}{{ 'a' | t }}{% endcomment %}{% raw %}{{ 'b' | t }}{% endraw %}{%- # {{ 'c' | t }} -%}").is_empty());
        assert_eq!(
            keys("{% liquid\n  # 'x' | t\n  echo 'y' | t\n  assign z = 'w' | t\n%}"),
            ["y", "w"]
        );
    }

    #[test]
    fn statements_of_liquid_tags_have_their_line() {
        let source = "a\n{% liquid\n  render 'one'\n  render 'two'\n%}\n{% render 'three' %}";
        let lines: Vec<(String, usize)> = statements(source)
            .iter()
            .map(|statement| (statement.markup.to_string(), statement.line))
            .collect();
        assert_eq!(
            lines,
            [
                ("'one'".to_string(), 3),
                ("'two'".to_string(), 4),
                ("'three'".to_string(), 6)
            ]
        );
    }
}
