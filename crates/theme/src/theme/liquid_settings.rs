//! The settings of type `liquid` stored in the JSON files of a theme: Liquid that merchants
//! write in the theme editor (a "Custom Liquid" section), which Shopify parses like a file.

use std::ops::Range;

use serde_json::Value as Json;

use super::Theme;
use super::schema::{SettingDef, parse_settings};

/// One value of a `liquid` setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiquidSetting {
    /// The keys that lead to the value from the root of the file.
    pub path: Vec<String>,
    pub value: String,
}

impl LiquidSetting {
    /// The path written with dots: `sections.main.settings.custom_liquid`.
    pub fn dotted(&self) -> String {
        self.path.join(".")
    }
}

/// Whether a JSON file of the theme can hold settings: templates, section groups and
/// `config/settings_data.json`.
pub fn holds_settings(path: &str) -> bool {
    path.ends_with(".json")
        && (path.starts_with("templates/")
            || (path.starts_with("sections/") && !path[9..].contains('/'))
            || path == "config/settings_data.json")
}

/// One value stored for a setting of a section or a block in a JSON file of the theme, with
/// its definition.
#[derive(Clone, Debug)]
pub struct StoredSetting {
    /// The keys that lead to the value from the root of the file.
    pub path: Vec<String>,
    pub def: SettingDef,
    pub value: Json,
}

impl StoredSetting {
    /// The path written with dots: `sections.main.settings.heading`.
    pub fn dotted(&self) -> String {
        self.path.join(".")
    }

    /// The path as a JSON pointer: `/sections/main/settings/heading`.
    pub fn pointer(&self) -> String {
        self.path
            .iter()
            .map(|key| format!("/{}", key.replace('~', "~0").replace('/', "~1")))
            .collect()
    }
}

/// What a walk over the stored settings calls for each value: its definition, the value, the
/// keys that lead to it.
type Visit<'v> = dyn FnMut(&SettingDef, &Json, Vec<String>) + 'v;

impl Theme {
    /// The values of the `liquid` settings in a JSON file of the theme, by what the schemas of
    /// its sections and blocks say (and `config/settings_schema.json` for the theme settings).
    pub fn liquid_settings(&self, path: &str) -> Vec<LiquidSetting> {
        let mut found = Vec::new();
        let mut visit = |def: &SettingDef, value: &Json, path: Vec<String>| {
            if def.kind == "liquid"
                && let Some(value) = value.as_str()
            {
                found.push(LiquidSetting {
                    path,
                    value: value.to_string(),
                });
            }
        };
        self.walk_settings(path, true, &mut visit);
        found
    }

    /// The values the sections and the blocks of a JSON file of the theme store for their
    /// settings (a template, a section group, the sections of `config/settings_data.json`),
    /// with their definitions. The theme settings are not among them.
    pub fn stored_settings(&self, path: &str) -> Vec<StoredSetting> {
        let mut found = Vec::new();
        let mut visit = |def: &SettingDef, value: &Json, path: Vec<String>| {
            found.push(StoredSetting {
                path,
                def: def.clone(),
                value: value.clone(),
            });
        };
        self.walk_settings(path, false, &mut visit);
        found
    }

    /// Calls `visit` for each value of a setting a JSON file stores: those of its sections and
    /// their blocks, and with `theme`, the theme settings of `config/settings_data.json`.
    fn walk_settings(&self, path: &str, theme: bool, visit: &mut Visit<'_>) {
        let Ok(Some(json)) = self.json(path) else {
            return;
        };
        if path == "config/settings_data.json" {
            let (key, current) = match json.get("current") {
                Some(Json::String(preset)) => (
                    vec!["presets".to_string(), preset.clone()],
                    json.get("presets").and_then(|presets| presets.get(preset)),
                ),
                other => (vec!["current".to_string()], other),
            };
            if let Some(current) = current {
                if theme {
                    let mut defs: Vec<SettingDef> = Vec::new();
                    if let Ok(Some(schema)) = self.json("config/settings_schema.json") {
                        for group in schema.as_array().into_iter().flatten() {
                            defs.extend(parse_settings(group.get("settings")));
                        }
                    }
                    // The theme settings sit directly in `current`.
                    collect(&defs, current, &key, visit);
                }
                self.sections(current, &key, visit);
            }
        } else {
            self.sections(&json, &[], visit);
        }
    }

    /// The settings of the sections of `parent.sections` and of their blocks.
    fn sections(&self, parent: &Json, path: &[String], visit: &mut Visit<'_>) {
        for (id, section) in parent
            .get("sections")
            .and_then(Json::as_object)
            .into_iter()
            .flatten()
        {
            let Some(kind) = section.get("type").and_then(Json::as_str) else {
                continue;
            };
            let schema = self.schema(&format!("sections/{kind}.liquid"));
            let path = extend(path, &["sections", id]);
            settings_of(&schema.settings, section, &path, visit);
            for (block_id, block) in blocks(section) {
                let Some(block_kind) = block.get("type").and_then(Json::as_str) else {
                    continue;
                };
                let block_path = extend(&path, &["blocks", block_id]);
                // A block the section declares, or a theme block.
                match schema.blocks.iter().find(|def| def.kind == block_kind) {
                    Some(def) => settings_of(&def.settings, block, &block_path, visit),
                    None => self.theme_block(block_kind, block, &block_path, visit),
                }
            }
        }
    }

    /// The settings of a theme block (`blocks/<kind>.liquid`) and of its own blocks.
    fn theme_block(&self, kind: &str, block: &Json, path: &[String], visit: &mut Visit<'_>) {
        // App blocks (`shopify://apps/...`) and `@app` have no file in the theme.
        if kind.contains('/') || kind.starts_with('@') {
            return;
        }
        let schema = self.schema(&format!("blocks/{kind}.liquid"));
        settings_of(&schema.settings, block, path, visit);
        for (id, child) in blocks(block) {
            if let Some(child_kind) = child.get("type").and_then(Json::as_str) {
                self.theme_block(child_kind, child, &extend(path, &["blocks", id]), visit);
            }
        }
    }
}

fn blocks(parent: &Json) -> impl Iterator<Item = (&String, &Json)> {
    parent
        .get("blocks")
        .and_then(Json::as_object)
        .into_iter()
        .flatten()
}

fn extend(path: &[String], keys: &[&str]) -> Vec<String> {
    let mut path = path.to_vec();
    path.extend(keys.iter().map(|key| key.to_string()));
    path
}

/// The settings of a section or a block, in its `settings`.
fn settings_of(defs: &[SettingDef], owner: &Json, path: &[String], visit: &mut Visit<'_>) {
    if let Some(settings) = owner.get("settings") {
        collect(defs, settings, &extend(path, &["settings"]), visit);
    }
}

/// The values of `settings` that have a definition.
fn collect(defs: &[SettingDef], settings: &Json, path: &[String], visit: &mut Visit<'_>) {
    for def in defs {
        if let Some(value) = settings.get(&def.id) {
            visit(def, value, extend(path, &[&def.id]));
        }
    }
}

/// Where the string at `path` is written in a JSON source that may hold comments: the range of
/// its literal, quotes included.
pub fn string_span(source: &str, path: &[String]) -> Option<Range<usize>> {
    let mut scanner = Scanner {
        bytes: source.as_bytes(),
        position: 0,
    };
    scanner.find(path)
}

struct Scanner<'s> {
    bytes: &'s [u8],
    position: usize,
}

impl Scanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    /// Skips whitespace and comments.
    fn skip_space(&mut self) {
        loop {
            match (self.peek(), self.bytes.get(self.position + 1)) {
                (Some(b' ' | b'\t' | b'\n' | b'\r'), _) => self.position += 1,
                (Some(b'/'), Some(b'/')) => {
                    while self.peek().is_some_and(|b| b != b'\n') {
                        self.position += 1;
                    }
                }
                (Some(b'/'), Some(b'*')) => {
                    self.position += 2;
                    while self.position < self.bytes.len()
                        && !self.bytes[self.position..].starts_with(b"*/")
                    {
                        self.position += 1;
                    }
                    self.position = (self.position + 2).min(self.bytes.len());
                }
                _ => return,
            }
        }
    }

    /// The end of the string literal that starts here.
    fn string(&mut self) -> Option<Range<usize>> {
        let start = self.position;
        if self.peek()? != b'"' {
            return None;
        }
        self.position += 1;
        loop {
            match self.peek()? {
                b'\\' => self.position += 2,
                b'"' => {
                    self.position += 1;
                    return Some(start..self.position);
                }
                _ => self.position += 1,
            }
        }
    }

    /// Skips the value that starts here.
    fn skip_value(&mut self) -> Option<()> {
        self.skip_space();
        match self.peek()? {
            b'"' => {
                self.string()?;
            }
            b'{' | b'[' => {
                let close = if self.peek()? == b'{' { b'}' } else { b']' };
                self.position += 1;
                loop {
                    self.skip_space();
                    match self.peek()? {
                        b if b == close => {
                            self.position += 1;
                            break;
                        }
                        b',' | b':' => self.position += 1,
                        _ => self.skip_value()?,
                    }
                }
            }
            _ => {
                while self.peek().is_some_and(|b| {
                    !matches!(b, b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t' | b'/')
                }) {
                    self.position += 1;
                }
            }
        }
        Some(())
    }

    /// Follows `path` from the value that starts here.
    fn find(&mut self, path: &[String]) -> Option<Range<usize>> {
        self.skip_space();
        let Some((key, rest)) = path.split_first() else {
            return self.string();
        };
        if self.peek()? != b'{' {
            return None;
        }
        self.position += 1;
        loop {
            self.skip_space();
            match self.peek()? {
                b'}' => return None,
                b',' => self.position += 1,
                _ => {
                    let literal = self.string()?;
                    let name: String = serde_json::from_slice(&self.bytes[literal]).ok()?;
                    self.skip_space();
                    if self.peek()? != b':' {
                        return None;
                    }
                    self.position += 1;
                    if &name == key {
                        return self.find(rest);
                    }
                    self.skip_value()?;
                }
            }
        }
    }
}

/// A string literal for `value`, escaped the way `like` (the literal it replaces) is: Shopify
/// writes `<`, `>` and `&` as `<`, `>` and `&`, and some tools write `/` as `\/`.
pub fn string_literal(value: &str, like: &str) -> String {
    let unicode_html =
        like.contains("\\u003c") || like.contains("\\u003e") || like.contains("\\u0026");
    let slash = like.contains("\\/");
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '/' if slash => out.push_str("\\/"),
            '<' | '>' | '&' if unicode_html => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|key| key.to_string()).collect()
    }

    #[test]
    fn finds_a_string_by_its_path() {
        let source = "/* comment */\n{\"a\": {\"b\": [1, {\"c\": \"}\"}], \"d\\\"\": \"x\", \"e\": {\"f\": \"y\\\"z\"}}}";
        let span = string_span(source, &path(&["a", "e", "f"])).unwrap();
        assert_eq!(&source[span], "\"y\\\"z\"");
        let span = string_span(source, &path(&["a", "d\""])).unwrap();
        assert_eq!(&source[span], "\"x\"");
        assert_eq!(string_span(source, &path(&["a", "b"])), None);
        assert_eq!(string_span(source, &path(&["z"])), None);
    }

    #[test]
    fn writes_literals_as_the_file_does() {
        assert_eq!(
            string_literal("<p>{{ a }}</p>\n", "\"\""),
            "\"<p>{{ a }}</p>\\n\""
        );
        assert_eq!(
            string_literal("<p>a & b</p>", "\"\\u003cp\\u003e\""),
            "\"\\u003cp\\u003ea \\u0026 b\\u003c/p\\u003e\""
        );
        assert_eq!(string_literal("a/b", "\"a\\/b\""), "\"a\\/b\"");
    }
}
