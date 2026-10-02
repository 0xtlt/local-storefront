//! `{% section 'name' %}` and `{% sections 'group' %}`.

use lsf_liquid::{Context, Error, Expr, Parser, Result, Tag, TagToken};
use serde_json::Value as Json;

use crate::render::section::{Placement, render_section};
use crate::render::state::RenderState;
use crate::theme::{BlockInstance, SectionInstance};
use crate::util::stable_id;

fn quoted_name(token: &TagToken<'_>) -> Result<String> {
    Expr::parse(token.markup)
        .as_literal_str()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            Error::syntax(format!(
                "Error in tag '{}' - Valid syntax: {} '[type]'",
                token.name, token.name
            ))
        })
}

struct StaticSection {
    name: String,
}

/// The data of a statically rendered section: what the theme editor stored for it, or the
/// defaults from its schema.
pub fn static_instance(state: &RenderState, name: &str) -> SectionInstance {
    let theme = &state.site.theme;
    let stored = theme
        .json("config/settings_data.json")
        .ok()
        .flatten()
        .and_then(|data| data.pointer(&format!("/current/sections/{name}")).cloned());
    if let Some(stored) = stored {
        let mut instance = SectionInstance::from_json(&stored);
        instance.kind = name.to_string();
        return instance;
    }
    let schema = theme.schema(&format!("sections/{name}.liquid"));
    let mut instance = SectionInstance {
        kind: name.to_string(),
        ..SectionInstance::default()
    };
    if let Some(default) = &schema.default {
        if let Some(settings) = default.get("settings").and_then(Json::as_object) {
            instance.settings = settings.clone();
        }
        // Default blocks are an array; they get positional ids.
        for (index, block) in default
            .get("blocks")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let id = format!("{name}-{index}");
            instance
                .blocks
                .insert(id.clone(), BlockInstance::from_json(block));
            instance.block_order.push(id);
        }
    }
    instance
}

impl Tag for StaticSection {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let state = RenderState::of(ctx)?;
        let instance = static_instance(state, &self.name);
        let placement = Placement {
            id: self.name.clone(),
            location: "static",
            group: None,
        };
        out.push_str(&render_section(ctx, &placement, &instance)?);
        Ok(())
    }
}

pub(super) fn parse_section(
    _parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    Ok(Box::new(StaticSection {
        name: quoted_name(token)?,
    }))
}

struct SectionGroup {
    name: String,
}

/// The id prefix of the sections of a group: `sections--<number>__`.
pub fn group_id_prefix(group: &str) -> String {
    format!("sections--{}__", stable_id("section_group", group))
}

/// Renders every section of a section group (`sections/<name>.json`).
pub fn render_group(ctx: &Context, name: &str) -> Result<String> {
    let state = RenderState::of(ctx)?;
    let path = format!("sections/{name}.json");
    let group = state
        .site
        .theme
        .template_json(&path)
        .map_err(|error| Error::standard(error.to_string()))?
        .ok_or_else(|| Error::standard(format!("Could not find asset {path}")))?;
    let location = group
        .group_type
        .clone()
        .unwrap_or_else(|| "custom".to_string());
    let prefix = group_id_prefix(name);
    let mut out = String::new();
    for key in &group.order {
        let Some(instance) = group.sections.get(key).filter(|section| !section.disabled) else {
            continue;
        };
        let placement = Placement {
            id: format!("{prefix}{key}"),
            location: &location,
            group: Some(name),
        };
        match render_section(ctx, &placement, instance) {
            Ok(html) => out.push_str(&html),
            Err(error) => out.push_str(&ctx.handle_error(error, 1)),
        }
    }
    Ok(out)
}

impl Tag for SectionGroup {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        out.push_str(&render_group(ctx, &self.name)?);
        Ok(())
    }
}

pub(super) fn parse_sections(
    _parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    Ok(Box::new(SectionGroup {
        name: quoted_name(token)?,
    }))
}
