//! JSON templates (`templates/*.json`) and section groups (`sections/*.json`), and the
//! contextual files that adapt them to a market or to B2B buyers
//! (`index.context.<context>.json`, `header-group.context.<context>.json`).

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

/// Who a page is rendered for, as Shopify picks a contextual template or section group: the
/// handle of the visitor's market (`localization.market.handle`) and whether they buy for a
/// company (`customer.b2b?`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuyerContext {
    pub market: Option<String>,
    pub b2b: bool,
}

/// Whether a file of `templates/` or `sections/` is a contextual one,
/// `<parent>.context.<context>.json`: overrides of its parent for a market or for B2B buyers,
/// not a template or a section group of its own.
pub fn is_contextual(file: &str) -> bool {
    file.strip_suffix(".json")
        .is_some_and(|stem| stem.contains(".context."))
}

/// The `context` of a contextual file: the market's handle it names and whether it is for B2B
/// buyers, why it is no context Shopify documents, or `None` when the file has none.
pub fn context_of(json: &Json) -> Option<Result<(Option<&str>, bool), String>> {
    let context = json.get("context")?;
    let Some(context) = context.as_object() else {
        return Some(Err("`context` is not an object".to_string()));
    };
    let mut market = None;
    let mut b2b = false;
    for (key, value) in context {
        match (key.as_str(), value) {
            ("market", Json::String(handle)) if !handle.is_empty() => {
                market = Some(handle.as_str())
            }
            ("b2b", Json::Bool(true)) => b2b = true,
            ("market", _) => return Some(Err("`context.market` is not a market's handle".into())),
            ("b2b", _) => return Some(Err("`context.b2b` is not `true`".into())),
            (other, _) => {
                return Some(Err(format!(
                    "`context.{other}` is no context: Shopify's are `market` and `b2b`"
                )));
            }
        }
    }
    if market.is_none() && !b2b {
        return Some(Err("`context` names neither a market nor `b2b`".to_string()));
    }
    Some(Ok((market, b2b)))
}

/// Whether a contextual file applies to a buyer: every key of its `context` matches.
pub fn applies_to(json: &Json, buyer: &BuyerContext) -> bool {
    match context_of(json) {
        Some(Ok((market, b2b))) => {
            market.is_none_or(|handle| {
                buyer
                    .market
                    .as_deref()
                    .is_some_and(|own| own.eq_ignore_ascii_case(handle))
            }) && (!b2b || buyer.b2b)
        }
        _ => false,
    }
}

/// Puts the overrides of a contextual file on its parent, as Shopify does: a setting, `disabled`,
/// a section's or a block's own keys replace the parent's one by one; `order` and `block_order`
/// replace the parent's order; a section or a block the parent does not have is added only
/// when the override gives its `type` (one the parent deleted stays deleted).
pub fn apply_overrides(parent: &mut Json, overrides: &Json) {
    let (Some(parent), Some(overrides)) = (parent.as_object_mut(), overrides.as_object()) else {
        return;
    };
    for (key, value) in overrides {
        if matches!(key.as_str(), "context" | "parent") {
            continue;
        }
        merge(parent, key, value);
    }
}

fn merge(into: &mut Map<String, Json>, key: &str, value: &Json) {
    match (into.get_mut(key), value) {
        (Some(Json::Object(existing)), Json::Object(overrides)) => {
            let instances = matches!(key, "sections" | "blocks");
            for (inner, value) in overrides {
                if instances && !existing.contains_key(inner) && value.get("type").is_none() {
                    continue;
                }
                merge(existing, inner, value);
            }
        }
        _ => {
            into.insert(key.to_string(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn overrides_are_put_on_the_parent_one_by_one() {
        let mut parent = json!({
            "sections": {
                "hero": {
                    "type": "banner",
                    "settings": { "text": "default", "size": "large" },
                    "blocks": {
                        "heading": { "type": "heading", "settings": { "size": "h1", "bold": true } },
                        "button": { "type": "button" }
                    },
                    "block_order": ["heading", "button"]
                },
                "gone": { "type": "banner" }
            },
            "order": ["hero", "gone"]
        });
        apply_overrides(
            &mut parent,
            &json!({
                "context": { "market": "ca" },
                "parent": "index.json",
                "sections": {
                    "hero": {
                        "settings": { "text": "ca" },
                        "blocks": {
                            "heading": { "disabled": true, "settings": { "size": "h2" } },
                            "added": { "type": "text" },
                            "deleted": { "settings": { "text": "x" } }
                        },
                        "block_order": ["button", "heading", "added", "deleted"]
                    },
                    "deleted": { "settings": { "text": "x" } },
                    "gone": { "disabled": true }
                },
                "order": ["gone", "hero", "deleted"]
            }),
        );
        let template = TemplateJson::from_json(&parent);
        assert_eq!(template.order, ["gone", "hero", "deleted"]);
        assert!(template.sections["gone"].disabled);
        assert!(!template.sections.contains_key("deleted"));
        let hero = &template.sections["hero"];
        assert_eq!(hero.settings["text"], "ca");
        assert_eq!(hero.settings["size"], "large");
        assert_eq!(hero.block_order, ["button", "heading", "added", "deleted"]);
        assert!(hero.blocks["heading"].disabled);
        assert_eq!(hero.blocks["heading"].settings["size"], "h2");
        assert_eq!(hero.blocks["heading"].settings["bold"], true);
        assert_eq!(hero.blocks["added"].kind, "text");
        assert!(!hero.blocks.contains_key("deleted"));
    }

    #[test]
    fn a_context_applies_to_its_market_or_to_b2b_buyers() {
        let europe = BuyerContext {
            market: Some("europe".to_string()),
            b2b: false,
        };
        let b2b = BuyerContext {
            market: Some("us".to_string()),
            b2b: true,
        };
        let market = json!({ "context": { "market": "europe" } });
        assert!(applies_to(&market, &europe));
        assert!(!applies_to(&market, &b2b));
        assert!(!applies_to(&market, &BuyerContext::default()));
        let companies = json!({ "context": { "b2b": true } });
        assert!(applies_to(&companies, &b2b));
        assert!(!applies_to(&companies, &europe));
        for wrong in [
            json!({}),
            json!({ "context": {} }),
            json!({ "context": { "b2b": false } }),
            json!({ "context": { "country": "BE" } }),
            json!({ "context": "europe" }),
        ] {
            assert!(!applies_to(&wrong, &europe), "{wrong}");
        }
    }

    #[test]
    fn contextual_files_are_named_after_their_context() {
        assert!(is_contextual("index.context.europe.json"));
        assert!(is_contextual("product.alt.context.b2b.json"));
        assert!(is_contextual("header-group.context.ca.json"));
        assert!(!is_contextual("index.json"));
        assert!(!is_contextual("page.context.json"));
        assert!(!is_contextual("index.context.europe.liquid"));
    }
}
