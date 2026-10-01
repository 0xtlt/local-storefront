//! The `{% schema %}` of sections and theme blocks, and the setting definitions in
//! `config/settings_schema.json`.

use serde_json::Value as Json;

/// One input setting (`{"type": "text", "id": "heading", "default": "Hello"}`).
#[derive(Clone, Debug)]
pub struct SettingDef {
    pub kind: String,
    pub id: String,
    pub default: Option<Json>,
}

/// A block type a section declares in its schema.
#[derive(Clone, Debug)]
pub struct BlockDef {
    pub kind: String,
    pub settings: Vec<SettingDef>,
}

#[derive(Clone, Debug)]
pub enum Wrapper {
    /// Wrap the output in this HTML element (`div` unless the schema says otherwise).
    Tag(String),
    /// `"tag": null`: no wrapper, the block renders its own root element.
    None,
}

#[derive(Clone, Debug)]
pub struct Schema {
    pub name: Option<String>,
    pub wrapper: Wrapper,
    pub class: Option<String>,
    pub settings: Vec<SettingDef>,
    pub blocks: Vec<BlockDef>,
    /// The settings and blocks of a statically rendered section that has no stored data.
    pub default: Option<Json>,
    pub raw: Json,
}

impl Default for Schema {
    fn default() -> Self {
        Schema {
            name: None,
            wrapper: Wrapper::Tag("div".to_string()),
            class: None,
            settings: Vec::new(),
            blocks: Vec::new(),
            default: None,
            raw: Json::Null,
        }
    }
}

/// Parses a list of setting definitions, skipping the entries that are only editor chrome
/// (`header`, `paragraph`): they have no `id`.
pub fn parse_settings(list: Option<&Json>) -> Vec<SettingDef> {
    list.and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(SettingDef {
                kind: entry.get("type")?.as_str()?.to_string(),
                id: entry.get("id")?.as_str()?.to_string(),
                default: entry.get("default").cloned(),
            })
        })
        .collect()
}

impl Schema {
    pub fn from_json(raw: Json) -> Schema {
        let wrapper = match raw.get("tag") {
            Some(Json::Null) => Wrapper::None,
            Some(Json::String(tag)) if !tag.is_empty() => Wrapper::Tag(tag.clone()),
            _ => Wrapper::Tag("div".to_string()),
        };
        let blocks = raw
            .get("blocks")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|block| {
                Some(BlockDef {
                    kind: block.get("type")?.as_str()?.to_string(),
                    settings: parse_settings(block.get("settings")),
                })
            })
            .collect();
        Schema {
            name: raw.get("name").and_then(Json::as_str).map(str::to_string),
            wrapper,
            class: raw.get("class").and_then(Json::as_str).map(str::to_string),
            settings: parse_settings(raw.get("settings")),
            blocks,
            default: raw.get("default").cloned(),
            raw,
        }
    }

    /// The settings of a block type declared by this (section) schema.
    pub fn block(&self, kind: &str) -> Option<&BlockDef> {
        self.blocks.iter().find(|block| block.kind == kind)
    }
}

/// Finds the first `{% name %}...{% endname %}` block in a Liquid source and returns its body.
pub fn extract_block<'s>(source: &'s str, name: &str) -> Option<&'s str> {
    let (_, body_start) = find_tag(source, name, 0)?;
    let (body_end, _) = find_tag(source, &format!("end{name}"), body_start)?;
    Some(&source[body_start..body_end])
}

/// Every `{% name %}...{% endname %}` body in a Liquid source.
pub fn extract_blocks<'s>(source: &'s str, name: &str) -> Vec<&'s str> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((_, body_start)) = find_tag(source, name, from) {
        let Some((body_end, after)) = find_tag(source, &format!("end{name}"), body_start) else {
            break;
        };
        out.push(&source[body_start..body_end]);
        from = after;
    }
    out
}

/// Locates `{%- name -%}` (with optional whitespace control) and returns its start and end.
fn find_tag(source: &str, name: &str, from: usize) -> Option<(usize, usize)> {
    let mut pos = from;
    while let Some(offset) = source[pos..].find("{%") {
        let start = pos + offset;
        let end = start + source[start..].find("%}")? + 2;
        let inner = source[start + 2..end - 2].trim_matches('-').trim();
        if inner == name {
            return Some((start, end));
        }
        pos = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_schema_block() {
        let source = "<div>{{ x }}</div>\n{%- schema -%}\n{\"name\": \"Hero\", \"tag\": null}\n{% endschema %}";
        let body = extract_block(source, "schema").unwrap();
        let schema = Schema::from_json(serde_json::from_str(body).unwrap());
        assert_eq!(schema.name.as_deref(), Some("Hero"));
        assert!(matches!(schema.wrapper, Wrapper::None));
        assert!(extract_block("{% assign schema = 1 %}", "schema").is_none());
    }

    #[test]
    fn extracts_every_stylesheet_block() {
        let source = "{% stylesheet %}a{% endstylesheet %} x {% stylesheet %}b{% endstylesheet %}";
        assert_eq!(extract_blocks(source, "stylesheet"), vec!["a", "b"]);
    }
}
