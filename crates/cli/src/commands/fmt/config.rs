//! Prettier's configuration, as far as it applies to Liquid: the options of `.prettierrc`,
//! the files `.prettierignore` leaves out, and the patterns both name files with.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use lsf_fmt::Options;
use serde_json::{Map, Value};

/// The configuration files that are read, in the order Prettier looks for them.
const CONFIG_FILES: [&str; 5] = [
    "package.json",
    ".prettierrc",
    ".prettierrc.json",
    ".prettierrc.yaml",
    ".prettierrc.yml",
];

/// The configuration files Prettier reads and this command cannot: they are programs.
const UNREADABLE_CONFIG_FILES: [&str; 9] = [
    ".prettierrc.js",
    ".prettierrc.mjs",
    ".prettierrc.cjs",
    ".prettierrc.ts",
    ".prettierrc.toml",
    ".prettierrc.json5",
    "prettier.config.js",
    "prettier.config.mjs",
    "prettier.config.cjs",
];

/// Every pattern `{a,b}` stands for.
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let mut depth = 0;
    let mut close = None;
    let mut commas = Vec::new();
    for (index, c) in pattern[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + index);
                    break;
                }
            }
            ',' if depth == 1 => commas.push(open + index),
            _ => {}
        }
    }
    let Some(close) = close.filter(|_| !commas.is_empty()) else {
        return vec![pattern.to_string()];
    };
    let mut alternatives = Vec::new();
    let mut start = open + 1;
    for end in commas.into_iter().chain([close]) {
        alternatives.push(&pattern[start..end]);
        start = end + 1;
    }
    alternatives
        .into_iter()
        .flat_map(|alternative| {
            expand_braces(&format!(
                "{}{alternative}{}",
                &pattern[..open],
                &pattern[close + 1..]
            ))
        })
        .collect()
}

/// Whether `text` matches a pattern without braces: `*` and `?` stay inside a path segment,
/// `**` crosses them.
fn matches_simple(pattern: &[u8], text: &[u8]) -> bool {
    match pattern {
        [] => text.is_empty(),
        [b'*', b'*', b'/', rest @ ..] => {
            matches_simple(rest, text)
                || text
                    .iter()
                    .enumerate()
                    .any(|(index, byte)| *byte == b'/' && matches_simple(rest, &text[index + 1..]))
        }
        [b'*', b'*', rest @ ..] => {
            (0..=text.len()).any(|index| matches_simple(rest, &text[index..]))
        }
        [b'*', rest @ ..] => {
            let segment = text
                .iter()
                .position(|byte| *byte == b'/')
                .unwrap_or(text.len());
            (0..=segment).any(|index| matches_simple(rest, &text[index..]))
        }
        [b'?', rest @ ..] => {
            text.first().is_some_and(|byte| *byte != b'/') && matches_simple(rest, &text[1..])
        }
        [b'[', rest @ ..] => {
            let Some(close) = rest
                .iter()
                .skip(1)
                .position(|byte| *byte == b']')
                .map(|index| index + 1)
            else {
                return text.first() == Some(&b'[') && matches_simple(rest, &text[1..]);
            };
            let (negated, class) = match &rest[..close] {
                [b'!' | b'^', class @ ..] => (true, class),
                class => (false, class),
            };
            let Some(byte) = text.first().filter(|byte| **byte != b'/') else {
                return false;
            };
            let mut found = false;
            let mut index = 0;
            while index < class.len() {
                if index + 2 < class.len() && class[index + 1] == b'-' {
                    found |= (class[index]..=class[index + 2]).contains(byte);
                    index += 3;
                } else {
                    found |= class[index] == *byte;
                    index += 1;
                }
            }
            found != negated && matches_simple(&rest[close + 1..], &text[1..])
        }
        [b'\\', literal, rest @ ..] | [literal, rest @ ..] => {
            text.first() == Some(literal) && matches_simple(rest, &text[1..])
        }
    }
}

/// Whether a path, relative and with `/` separators, matches a glob pattern.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    expand_braces(pattern)
        .iter()
        .any(|pattern| matches_simple(pattern.as_bytes(), path.as_bytes()))
}

/// Whether one of the `files` patterns of an override names `path`, which is relative to the
/// configuration file. A pattern without a `/` names a file in any directory.
fn override_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    if pattern.contains('/') {
        glob_match(pattern, path)
    } else {
        glob_match(pattern, path.rsplit('/').next().unwrap_or(path))
    }
}

/// One line of a `.prettierignore`, which reads as a `.gitignore`.
struct IgnoreRule {
    pattern: String,
    negated: bool,
    directory_only: bool,
    /// The pattern is relative to the ignore file rather than to any directory.
    anchored: bool,
}

pub struct IgnoreFile {
    directory: PathBuf,
    rules: Vec<IgnoreRule>,
}

impl IgnoreFile {
    pub fn parse(directory: &Path, text: &str) -> IgnoreFile {
        let rules = text
            .lines()
            .map(|line| line.trim_end())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                let (negated, line) = match line.strip_prefix('!') {
                    Some(line) => (true, line),
                    None => (false, line),
                };
                let (directory_only, line) = match line.strip_suffix('/') {
                    Some(line) => (true, line),
                    None => (false, line),
                };
                let anchored = line.contains('/');
                IgnoreRule {
                    pattern: line.trim_start_matches('/').to_string(),
                    negated,
                    directory_only,
                    anchored,
                }
            })
            .collect();
        IgnoreFile {
            directory: directory.to_path_buf(),
            rules,
        }
    }

    /// The `.prettierignore` of `directory`, or of the nearest directory above it that has one.
    pub fn find(directory: &Path) -> Option<IgnoreFile> {
        directory.ancestors().find_map(|ancestor| {
            let text = fs::read_to_string(ancestor.join(".prettierignore")).ok()?;
            Some(IgnoreFile::parse(ancestor, &text))
        })
    }

    /// Whether the file or the directory at `path` is left out. The last rule that names it
    /// decides.
    pub fn ignores(&self, path: &Path, is_directory: bool) -> bool {
        let Some(relative) = relative_path(&self.directory, path) else {
            return false;
        };
        let name = relative.rsplit('/').next().unwrap_or(&relative);
        let mut ignored = false;
        for rule in &self.rules {
            if rule.directory_only && !is_directory {
                continue;
            }
            let matches = if rule.anchored {
                glob_match(&rule.pattern, &relative)
            } else {
                glob_match(&rule.pattern, name)
            };
            if matches {
                ignored = !rule.negated;
            }
        }
        ignored
    }
}

/// `path` relative to `base`, with `/` separators.
fn relative_path(base: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(base).ok()?;
    Some(
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// A scalar of the YAML a `.prettierrc` is written in.
fn yaml_scalar(text: &str) -> Value {
    let text = text.trim();
    if let Some(inner) = text
        .strip_prefix('[')
        .and_then(|text| text.strip_suffix(']'))
    {
        return Value::Array(
            inner
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(yaml_scalar)
                .collect(),
        );
    }
    for quote in ['"', '\''] {
        if let Some(inner) = text
            .strip_prefix(quote)
            .and_then(|text| text.strip_suffix(quote))
        {
            return Value::String(inner.to_string());
        }
    }
    match text {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" | "~" | "" => Value::Null,
        _ => text
            .parse::<u64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(text.to_string())),
    }
}

/// Reads the subset of YAML a Prettier configuration is written in: mappings, lists of
/// mappings or of scalars, and scalars, nested by indentation.
fn parse_yaml(text: &str) -> Result<Value, String> {
    // (indentation, text) of the lines that hold something.
    let lines: Vec<(usize, &str)> = text
        .lines()
        .map(|line| match line.find(" #") {
            Some(comment) => &line[..comment],
            None => line,
        })
        .filter(|line| {
            !line.trim().is_empty() && !line.trim_start().starts_with('#') && line.trim() != "---"
        })
        .map(|line| (line.len() - line.trim_start().len(), line.trim()))
        .collect();
    let mut index = 0;
    let value = parse_yaml_block(&lines, &mut index, 0)?;
    if index < lines.len() {
        return Err(format!("cannot read the line `{}`", lines[index].1));
    }
    Ok(value)
}

fn parse_yaml_block(
    lines: &[(usize, &str)],
    index: &mut usize,
    indent: usize,
) -> Result<Value, String> {
    let Some(&(first_indent, first)) = lines.get(*index) else {
        return Ok(Value::Null);
    };
    if first_indent < indent {
        return Ok(Value::Null);
    }
    let indent = first_indent;
    if first.starts_with("- ") || first == "-" {
        let mut items = Vec::new();
        while let Some(&(line_indent, line)) = lines.get(*index) {
            if line_indent != indent || !(line.starts_with("- ") || line == "-") {
                break;
            }
            let rest = line[1..].trim_start();
            let item_indent = indent + (line.len() - rest.len());
            if rest.is_empty() {
                *index += 1;
                items.push(parse_yaml_block(lines, index, indent + 1)?);
            } else if yaml_key(rest).is_some() {
                // A mapping that starts on the line of the dash: read it as if the dash were
                // indentation.
                let mut item_lines = vec![(item_indent, rest)];
                let mut end = *index + 1;
                while end < lines.len() && lines[end].0 > indent {
                    item_lines.push(lines[end]);
                    end += 1;
                }
                let mut item_index = 0;
                items.push(parse_yaml_block(&item_lines, &mut item_index, item_indent)?);
                *index = end;
            } else {
                *index += 1;
                items.push(yaml_scalar(rest));
            }
        }
        return Ok(Value::Array(items));
    }
    let mut map = Map::new();
    while let Some(&(line_indent, line)) = lines.get(*index) {
        if line_indent != indent {
            break;
        }
        let Some((key, rest)) = yaml_key(line) else {
            return Err(format!("cannot read the line `{line}`"));
        };
        *index += 1;
        let value = if rest.is_empty() {
            match lines.get(*index) {
                Some(&(next_indent, next))
                    if next_indent > indent || (next_indent == indent && next.starts_with('-')) =>
                {
                    parse_yaml_block(lines, index, next_indent)?
                }
                _ => Value::Null,
            }
        } else {
            yaml_scalar(rest)
        };
        map.insert(key.to_string(), value);
    }
    Ok(Value::Object(map))
}

/// `key: value` → the key, and what follows the colon.
fn yaml_key(line: &str) -> Option<(&str, &str)> {
    let (key, rest) = line.split_once(':')?;
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let key = key.trim().trim_matches(['"', '\'']);
    (!key.is_empty() && !key.contains(' ')).then_some((key, rest.trim()))
}

/// The options that apply to some files only.
struct Override {
    files: Vec<String>,
    excluded: Vec<String>,
    options: Map<String, Value>,
}

/// A configuration, read from one file.
struct Config {
    directory: PathBuf,
    options: Map<String, Value>,
    overrides: Vec<Override>,
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(pattern)) => vec![pattern.clone()],
        Some(Value::Array(patterns)) => patterns
            .iter()
            .filter_map(|pattern| pattern.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

impl Config {
    fn from_value(directory: &Path, value: Value) -> Config {
        let mut options = match value {
            Value::Object(options) => options,
            _ => Map::new(),
        };
        let overrides = match options.remove("overrides") {
            Some(Value::Array(overrides)) => overrides
                .into_iter()
                .filter_map(|entry| {
                    Some(Override {
                        files: string_list(entry.get("files")),
                        excluded: string_list(entry.get("excludeFiles")),
                        options: entry.get("options")?.as_object()?.clone(),
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        Config {
            directory: directory.to_path_buf(),
            options,
            overrides,
        }
    }

    /// The configuration of `directory`, if one of the files there holds one.
    fn read(directory: &Path) -> Result<Option<Config>, String> {
        for name in CONFIG_FILES {
            let path = directory.join(name);
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let invalid = |error: String| format!("{}: {error}", path.display());
            let value = if name == "package.json" {
                let package: Value =
                    serde_json::from_str(&text).map_err(|error| invalid(error.to_string()))?;
                match package.get("prettier") {
                    Some(config @ Value::Object(_)) => config.clone(),
                    // No configuration there, or the name of a package that holds one.
                    _ => continue,
                }
            } else if name.ends_with(".json") || text.trim_start().starts_with('{') {
                serde_json::from_str(&text).map_err(|error| invalid(error.to_string()))?
            } else {
                parse_yaml(&text).map_err(invalid)?
            };
            return Ok(Some(Config::from_value(directory, value)));
        }
        Ok(None)
    }

    /// Sets the options the configuration gives `file`.
    fn apply(&self, options: &mut Options, file: &Path) -> Result<(), String> {
        let mut apply = |values: &Map<String, Value>| -> Result<(), String> {
            for (name, value) in values {
                options
                    .set(name, value)
                    .map_err(|error| format!("{}: {error}", self.directory.display()))?;
            }
            Ok(())
        };
        apply(&self.options)?;
        let relative = relative_path(&self.directory, file).unwrap_or_default();
        for entry in &self.overrides {
            let named = |patterns: &[String]| {
                patterns
                    .iter()
                    .any(|pattern| override_matches(pattern, &relative))
            };
            if named(&entry.files) && !named(&entry.excluded) {
                apply(&entry.options)?;
            }
        }
        Ok(())
    }
}

/// An `.editorconfig`: sections of properties, each for the files a pattern names.
struct EditorConfig {
    directory: PathBuf,
    /// The search for more of them stops at this one.
    root: bool,
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl EditorConfig {
    fn parse(directory: &Path, text: &str) -> EditorConfig {
        let mut config = EditorConfig {
            directory: directory.to_path_buf(),
            root: false,
            sections: Vec::new(),
        };
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            if let Some(pattern) = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
            {
                config.sections.push((pattern.to_string(), Vec::new()));
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim().to_lowercase(), value.trim().to_lowercase());
            match config.sections.last_mut() {
                Some((_, properties)) => properties.push((key, value)),
                None if key == "root" => config.root = value == "true",
                None => {}
            }
        }
        config
    }

    /// The properties the file is given, a later section winning over an earlier one.
    fn properties_for(&self, file: &Path, properties: &mut HashMap<String, String>) {
        let Some(relative) = relative_path(&self.directory, file) else {
            return;
        };
        for (pattern, section) in &self.sections {
            // A pattern without a `/` names files in any directory.
            let matches = match pattern.strip_prefix('/') {
                Some(anchored) => glob_match(anchored, &relative),
                None if pattern.contains('/') => glob_match(pattern, &relative),
                None => glob_match(&format!("**/{pattern}"), &relative),
            };
            if matches {
                for (key, value) in section {
                    properties.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

/// What Prettier makes of the properties of an `.editorconfig`.
fn editorconfig_to_prettier(properties: &HashMap<String, String>) -> Map<String, Value> {
    let get = |key: &str| {
        properties
            .get(key)
            .map(String::as_str)
            .filter(|value| *value != "unset")
    };
    let number = |key: &str| get(key).and_then(|value| value.parse::<u64>().ok());
    let mut options = Map::new();
    let indent_style = get("indent_style");
    let mut use_tabs = indent_style.map(|style| style == "tab");
    if get("indent_size") == Some("tab") {
        use_tabs = Some(true);
    }
    if let Some(use_tabs) = use_tabs {
        options.insert("useTabs".into(), use_tabs.into());
    }
    let tab_width = match (use_tabs, number("tab_width"), number("indent_size")) {
        (Some(true), Some(tab_width), _) => Some(tab_width),
        (_, _, Some(indent_size)) if indent_style != Some("tab") => Some(indent_size),
        (_, tab_width, _) => tab_width,
    };
    if let Some(tab_width) = tab_width {
        options.insert("tabWidth".into(), tab_width.into());
    }
    match (get("max_line_length"), number("max_line_length")) {
        (_, Some(width)) => {
            options.insert("printWidth".into(), width.into());
        }
        // No limit: a line longer than this does not exist.
        (Some("off"), _) => {
            options.insert("printWidth".into(), 1_000_000.into());
        }
        _ => {}
    }
    match get("quote_type") {
        Some("single") => {
            options.insert("singleQuote".into(), true.into());
        }
        Some("double") => {
            options.insert("singleQuote".into(), false.into());
        }
        _ => {}
    }
    if let Some(end_of_line @ ("lf" | "crlf" | "cr")) = get("end_of_line") {
        options.insert("endOfLine".into(), end_of_line.into());
    }
    options
}

/// Finds the configuration that applies to each file, as Prettier does: the `.editorconfig`
/// files above it, then the Prettier configuration of the nearest directory that has one.
#[derive(Default)]
pub struct Configs {
    by_directory: HashMap<PathBuf, Option<Rc<Config>>>,
    editor_configs: HashMap<PathBuf, Option<Rc<EditorConfig>>>,
    /// The configuration files met that cannot be read.
    pub unreadable: Vec<PathBuf>,
}

impl Configs {
    fn config_of(&mut self, directory: &Path) -> Result<Option<Rc<Config>>, String> {
        if let Some(config) = self.by_directory.get(directory) {
            return Ok(config.clone());
        }
        for name in UNREADABLE_CONFIG_FILES {
            let path = directory.join(name);
            if path.is_file() && !self.unreadable.contains(&path) {
                self.unreadable.push(path);
            }
        }
        let config = match Config::read(directory)? {
            Some(config) => Some(Rc::new(config)),
            None => match directory.parent() {
                Some(parent) => self.config_of(parent)?,
                None => None,
            },
        };
        self.by_directory
            .insert(directory.to_path_buf(), config.clone());
        Ok(config)
    }

    fn editor_config_of(&mut self, directory: &Path) -> Option<Rc<EditorConfig>> {
        if let Some(config) = self.editor_configs.get(directory) {
            return config.clone();
        }
        let config = fs::read_to_string(directory.join(".editorconfig"))
            .ok()
            .map(|text| Rc::new(EditorConfig::parse(directory, &text)));
        self.editor_configs
            .insert(directory.to_path_buf(), config.clone());
        config
    }

    /// The options the `.editorconfig` files above `file` give it.
    fn editor_options_for(&mut self, file: &Path) -> Map<String, Value> {
        let mut configs = Vec::new();
        for directory in file.ancestors().skip(1) {
            if let Some(config) = self.editor_config_of(directory) {
                let root = config.root;
                configs.push(config);
                if root {
                    break;
                }
            }
        }
        // The nearest file wins: it is read last.
        let mut properties = HashMap::new();
        for config in configs.iter().rev() {
            config.properties_for(file, &mut properties);
        }
        editorconfig_to_prettier(&properties)
    }

    /// The options `file` is formatted with.
    pub fn options_for(&mut self, file: &Path) -> Result<Options, String> {
        let mut options = Options::default();
        for (name, value) in self.editor_options_for(file) {
            options
                .set(&name, &value)
                .map_err(|error| format!(".editorconfig: {error}"))?;
        }
        let directory = file.parent().unwrap_or(Path::new("."));
        if let Some(config) = self.config_of(directory)? {
            config.apply(&mut options, file)?;
        }
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_as_prettier_and_git_read_them() {
        assert!(glob_match("*.liquid", "a.liquid"));
        assert!(!glob_match("*.liquid", "sections/a.liquid"));
        assert!(glob_match("**/*.liquid", "sections/a.liquid"));
        assert!(glob_match("**/*.liquid", "a.liquid"));
        assert!(glob_match("sections/**", "sections/a/b.liquid"));
        assert!(glob_match("*.{liquid,json}", "a.json"));
        assert!(glob_match("a[0-9]?.liquid", "a1x.liquid"));
        assert!(!glob_match("a[!0-9].liquid", "a1.liquid"));
        assert!(override_matches("*.liquid", "sections/a.liquid"));
        assert!(!override_matches("snippets/*.liquid", "sections/a.liquid"));
    }

    #[test]
    fn an_ignore_file_reads_as_a_gitignore() {
        let ignore = IgnoreFile::parse(
            Path::new("/theme"),
            "# vendored\nassets/\n*.min.liquid\n/snippets/icon-*.liquid\n!snippets/icon-keep.liquid\n",
        );
        assert!(ignore.ignores(Path::new("/theme/assets"), true));
        assert!(!ignore.ignores(Path::new("/theme/assets"), false));
        assert!(ignore.ignores(Path::new("/theme/sections/a.min.liquid"), false));
        assert!(ignore.ignores(Path::new("/theme/snippets/icon-a.liquid"), false));
        assert!(!ignore.ignores(Path::new("/theme/snippets/icon-keep.liquid"), false));
        assert!(!ignore.ignores(Path::new("/theme/sections/icon-a.liquid"), false));
    }

    #[test]
    fn reads_a_configuration_written_in_yaml() {
        let yaml = "# comment\nprintWidth: 100\nsingleQuote: true\noverrides:\n  - files: \"*.liquid\"\n    options:\n      singleQuote: false\n      printWidth: 90\n  - files:\n      - a.liquid\n      - b.liquid\n    excludeFiles: [b.liquid]\n    options:\n      useTabs: true\n";
        let value = parse_yaml(yaml).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "printWidth": 100,
                "singleQuote": true,
                "overrides": [
                    { "files": "*.liquid", "options": { "singleQuote": false, "printWidth": 90 } },
                    { "files": ["a.liquid", "b.liquid"], "excludeFiles": ["b.liquid"], "options": { "useTabs": true } },
                ],
            })
        );
    }

    #[test]
    fn overrides_apply_to_the_files_they_name() {
        let config = Config::from_value(
            Path::new("/theme"),
            serde_json::json!({
                "printWidth": 100,
                "singleQuote": true,
                "semi": false,
                "overrides": [
                    { "files": "*.liquid", "options": { "singleQuote": false } },
                    { "files": "snippets/*.liquid", "excludeFiles": "snippets/b.liquid", "options": { "printWidth": 60 } },
                ],
            }),
        );
        let options_for = |file: &str| {
            let mut options = Options::default();
            config
                .apply(&mut options, Path::new(file))
                .map(|()| options)
        };
        let options = options_for("/theme/sections/a.liquid").unwrap();
        assert_eq!((options.print_width, options.single_quote), (100, false));
        let options = options_for("/theme/snippets/a.liquid").unwrap();
        assert_eq!(options.print_width, 60);
        let options = options_for("/theme/snippets/b.liquid").unwrap();
        assert_eq!(options.print_width, 100);

        let invalid = Config::from_value(
            Path::new("/theme"),
            serde_json::json!({ "printWidth": "wide" }),
        );
        assert!(
            invalid
                .apply(&mut Options::default(), Path::new("/theme/a.liquid"))
                .is_err()
        );
    }

    #[test]
    fn an_editorconfig_gives_what_prettier_takes_from_it() {
        let config = EditorConfig::parse(
            Path::new("/theme"),
            "root = true\n\n[*]\nindent_style = space\nindent_size = 4\nmax_line_length = 100\n\n[*.liquid]\nindent_size = 2\nend_of_line = CRLF\n\n[assets/**]\nindent_style = tab\ntab_width = 8\n",
        );
        assert!(config.root);
        let options_for = |file: &str| {
            let mut properties = HashMap::new();
            config.properties_for(Path::new(file), &mut properties);
            Value::Object(editorconfig_to_prettier(&properties))
        };
        assert_eq!(
            options_for("/theme/sections/a.liquid"),
            serde_json::json!({ "useTabs": false, "tabWidth": 2, "printWidth": 100, "endOfLine": "crlf" })
        );
        assert_eq!(
            options_for("/theme/assets/a.css"),
            serde_json::json!({ "useTabs": true, "tabWidth": 8, "printWidth": 100 })
        );
    }
}
