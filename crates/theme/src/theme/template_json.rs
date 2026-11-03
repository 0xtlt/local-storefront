//! JSON templates (`templates/*.json`) and section groups (`sections/*.json`).

use indexmap::IndexMap;
use serde_json::{Map, Value as Json};

/// A block inside a section or inside another block.
#[derive(Clone, Debug, Default)]
pub struct BlockInstance {
    pub kind: String,
    pub disabled: bool,
    /// A block rendered by `{% content_for 'block', id: ... %}` rather than listed in the order.
    pub is_static: bool,
    pub settings: Map<String, Json>,
    pub blocks: IndexMap<String, BlockInstance>,
    pub block_order: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SectionInstance {
    pub kind: String,
    pub disabled: bool,
    pub settings: Map<String, Json>,
    pub blocks: IndexMap<String, BlockInstance>,
    pub block_order: Vec<String>,
    pub custom_css: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    /// `layout/theme.liquid`.
    Default,
    /// `"layout": "name"` → `layout/name.liquid`.
    Named(String),
    /// `"layout": false`: render the sections without a layout.
    None,
}

#[derive(Clone, Debug)]
pub struct TemplateJson {
    pub layout: Layout,
    /// `"wrapper": "div#id.class"`, an element wrapped around all the sections.
    pub wrapper: Option<String>,
    /// For section groups: `header`, `footer`, `aside` or `custom.<name>`.
    pub group_type: Option<String>,
    pub sections: IndexMap<String, SectionInstance>,
    pub order: Vec<String>,
}

fn string_list(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_blocks(value: Option<&Json>) -> IndexMap<String, BlockInstance> {
    value
        .and_then(Json::as_object)
        .map(|blocks| {
            blocks
                .iter()
                .map(|(id, block)| (id.clone(), BlockInstance::from_json(block)))
                .collect()
        })
        .unwrap_or_default()
}

impl BlockInstance {
    pub fn from_json(json: &Json) -> BlockInstance {
        BlockInstance {
            kind: json
                .get("type")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            disabled: json
                .get("disabled")
                .and_then(Json::as_bool)
                .unwrap_or(false),
            is_static: json.get("static").and_then(Json::as_bool).unwrap_or(false),
            settings: json
                .get("settings")
                .and_then(Json::as_object)
                .cloned()
                .unwrap_or_default(),
            blocks: parse_blocks(json.get("blocks")),
            block_order: string_list(json.get("block_order")),
        }
    }
}

impl SectionInstance {
    pub fn from_json(json: &Json) -> SectionInstance {
        SectionInstance {
            kind: json
                .get("type")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            disabled: json
                .get("disabled")
                .and_then(Json::as_bool)
                .unwrap_or(false),
            settings: json
                .get("settings")
                .and_then(Json::as_object)
                .cloned()
                .unwrap_or_default(),
            blocks: parse_blocks(json.get("blocks")),
            block_order: string_list(json.get("block_order")),
            custom_css: string_list(json.get("custom_css")),
        }
    }
}

impl TemplateJson {
    pub fn from_json(json: &Json) -> TemplateJson {
        let layout = match json.get("layout") {
            Some(Json::Bool(false)) => Layout::None,
            Some(Json::String(name)) if !name.is_empty() => Layout::Named(name.clone()),
            _ => Layout::Default,
        };
        let sections: IndexMap<String, SectionInstance> = json
            .get("sections")
            .and_then(Json::as_object)
            .map(|sections| {
                sections
                    .iter()
                    .map(|(id, section)| (id.clone(), SectionInstance::from_json(section)))
                    .collect()
            })
            .unwrap_or_default();
        let order = match json.get("order") {
            Some(order) => string_list(Some(order)),
            None => sections.keys().cloned().collect(),
        };
        TemplateJson {
            layout,
            wrapper: json
                .get("wrapper")
                .and_then(Json::as_str)
                .map(str::to_string),
            group_type: json.get("type").and_then(Json::as_str).map(str::to_string),
            sections,
            order,
        }
    }
}
