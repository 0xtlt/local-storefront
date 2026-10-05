//! Rendering sections and blocks: resolving their settings, building the `section` and `block`
//! objects, and wrapping the output the way Shopify does.

use std::any::Any;
use std::sync::Arc;
use std::time::Instant;

use indexmap::IndexMap;
use lsf_liquid::{Context, Error, Hash, Object, Result, Value};

use super::settings;
use super::state::RenderState;
use crate::theme::{BlockInstance, Schema, SectionInstance, Wrapper};
use crate::util::short_hash;

/// The context variable holding the [`Container`] `content_for` renders blocks from. The name
/// cannot be written in Liquid, so templates cannot see or shadow it.
pub const CONTAINER_VARIABLE: &str = "\u{0}container";

/// Where `content_for` finds the blocks to render: the section or block being rendered.
pub struct Container {
    pub section_id: String,
    pub section: Value,
    pub blocks: IndexMap<String, BlockInstance>,
    pub block_order: Vec<String>,
    /// The resources `closest.<type>` resolves to at this point of the tree.
    pub closest: Arc<Hash>,
}

impl Object for Container {
    fn type_name(&self) -> &str {
        "container"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct SectionDrop {
    pub id: String,
    pub kind: String,
    pub settings: Value,
    pub blocks: Value,
    pub index: usize,
    pub location: String,
}

impl Object for SectionDrop {
    fn type_name(&self) -> &str {
        "section"
    }

    fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "id" => Value::from(&self.id),
            "type" => Value::from(&self.kind),
            "settings" => self.settings.clone(),
            "blocks" => self.blocks.clone(),
            "index" => Value::from(self.index),
            "index0" => Value::from(self.index.saturating_sub(1)),
            "location" => Value::from(&self.location),
            _ => return None,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("section:{}", self.id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct BlockDrop {
    pub id: String,
    pub kind: String,
    pub settings: Value,
}

impl Object for BlockDrop {
    fn type_name(&self) -> &str {
        "block"
    }

    fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "id" => Value::from(&self.id),
            "type" => Value::from(&self.kind),
            "settings" => self.settings.clone(),
            // Only the theme editor needs these attributes; the storefront gets none.
            "shopify_attributes" => Value::empty_string(),
            _ => return None,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("block:{}", self.id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `closest.<type>`: the nearest resource of a type, looking up the block tree and falling
/// back to the resource of the page.
pub struct ClosestDrop(pub Arc<Hash>);

impl Object for ClosestDrop {
    fn type_name(&self) -> &str {
        "closest"
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.0.get(key).cloned()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The id of a theme block: unique across the page, because it embeds its section.
fn theme_block_id(section_id: &str, key: &str) -> String {
    format!("A{}__{key}", &short_hash(section_id)[..15])
}

fn is_app_block(kind: &str) -> bool {
    kind.starts_with("shopify://apps/") || kind == "@app"
}

/// Whether a block type is a theme block (`blocks/<type>.liquid`) rather than one defined
/// inline by the section's schema.
fn is_theme_block(state: &RenderState, section_schema: &Schema, kind: &str) -> bool {
    section_schema
        .block(kind)
        .is_none_or(|def| def.settings.is_empty())
        && state
            .site
            .theme
            .files()
            .exists(&format!("blocks/{kind}.liquid"))
}

/// The page-level resources `closest` falls back to.
pub fn page_closest(ctx: &Context) -> Arc<Hash> {
    let mut closest = Hash::new();
    for name in ["product", "collection", "article", "blog", "page"] {
        let value = ctx.find_variable(name);
        if !value.is_nil() {
            closest.insert(name.to_string(), value);
        }
    }
    Arc::new(closest)
}

/// The blocks listed in `section.blocks`, in order.
fn section_blocks(
    state: &RenderState,
    ctx: &Context,
    section_id: &str,
    schema: &Schema,
    instance: &SectionInstance,
    closest: &Arc<Hash>,
) -> Value {
    let theme = &state.site.theme;
    let closest_value = Value::object(ClosestDrop(closest.clone()));
    let blocks = instance
        .block_order
        .iter()
        .filter_map(|key| instance.blocks.get(key).map(|block| (key, block)))
        .filter(|(_, block)| !block.disabled && !is_app_block(&block.kind))
        .map(|(key, block)| {
            let theme_block = is_theme_block(state, schema, &block.kind);
            let (id, defs) = if theme_block {
                (
                    theme_block_id(section_id, key),
                    theme
                        .schema(&format!("blocks/{}.liquid", block.kind))
                        .settings
                        .clone(),
                )
            } else {
                (
                    key.clone(),
                    schema
                        .block(&block.kind)
                        .map(|def| def.settings.clone())
                        .unwrap_or_default(),
                )
            };
            Value::object(BlockDrop {
                id,
                kind: block.kind.clone(),
                settings: settings::resolve(
                    state,
                    ctx,
                    &defs,
                    &block.settings,
                    &[("closest", closest_value.clone())],
                ),
            })
        })
        .collect();
    Value::array(blocks)
}

/// How a section is placed on the page.
pub struct Placement<'a> {
    /// The unique id: `template--123__main`, `sections--456__header`, or a static name.
    pub id: String,
    /// `template`, `header`, `footer`, `aside`, `custom.<name>` or `static`.
    pub location: &'a str,
    /// The section group file name, for the wrapper's class.
    pub group: Option<&'a str>,
}

/// Renders a section: its Liquid, wrapped in `<div id="shopify-section-...">`.
pub fn render_section(
    ctx: &Context,
    placement: &Placement<'_>,
    instance: &SectionInstance,
) -> Result<String> {
    let started = Instant::now();
    // In a profile, the section as it is placed on the page, around the file of its type.
    let _span = ctx
        .profiler()
        .map(|profiler| profiler.span(&format!("section {}", placement.id), None));
    let state = RenderState::of(ctx)?;
    let blocks_before = state.blocks_so_far();
    let theme = &state.site.theme;
    let path = format!("sections/{}.liquid", instance.kind);
    let file = theme.liquid(&path)?.ok_or_else(|| {
        Error::standard(format!(
            "Error in tag 'section' - '{}' is not a valid section type",
            instance.kind
        ))
    })?;
    let schema = &file.schema;
    let closest = page_closest(ctx);
    let section = Value::object(SectionDrop {
        id: placement.id.clone(),
        kind: instance.kind.clone(),
        settings: settings::resolve(state, ctx, &schema.settings, &instance.settings, &[]),
        blocks: section_blocks(state, ctx, &placement.id, schema, instance, &closest),
        index: state.next_section_index(placement.location),
        location: placement.location.to_string(),
    });
    let mut inner = ctx.isolated()?;
    // Snippets rendered from the section see `section` without receiving it as an argument.
    inner.set_inherited("section", section.clone());
    inner.set_inherited("block", Value::Nil);
    inner.set_inherited("closest", Value::object(ClosestDrop(closest.clone())));
    inner.set(
        CONTAINER_VARIABLE,
        Value::object(Container {
            section_id: placement.id.clone(),
            section,
            blocks: instance.blocks.clone(),
            block_order: instance.block_order.clone(),
            closest,
        }),
    );
    let content = file.template.render(&mut inner);
    state.record_section(
        &placement.id,
        &instance.kind,
        started.elapsed(),
        blocks_before,
    );

    let tag = match &schema.wrapper {
        Wrapper::Tag(tag) => tag.as_str(),
        Wrapper::None => "div",
    };
    let mut classes = String::from("shopify-section");
    if let Some(group) = placement.group {
        classes.push_str(" shopify-section-group-");
        classes.push_str(group);
    }
    if let Some(class) = schema.class.as_deref().filter(|class| !class.is_empty()) {
        classes.push(' ');
        classes.push_str(class);
    }
    Ok(format!(
        "<{tag} id=\"shopify-section-{}\" class=\"{classes}\">{content}</{tag}>",
        placement.id
    ))
}

/// Renders one theme block (`blocks/<type>.liquid`) of a container.
pub fn render_block(
    ctx: &Context,
    container: &Container,
    key: &str,
    instance: &BlockInstance,
    closest: &Arc<Hash>,
    variables: &[(String, Value)],
) -> Result<String> {
    if is_app_block(&instance.kind) {
        return Ok(String::new());
    }
    let started = Instant::now();
    let _span = ctx
        .profiler()
        .map(|profiler| profiler.span(&format!("block {key}"), None));
    let state = RenderState::of(ctx)?;
    let path = format!("blocks/{}.liquid", instance.kind);
    let file = state
        .site
        .theme
        .liquid(&path)?
        .ok_or_else(|| Error::standard(format!("Could not find asset {path}")))?;
    let schema = &file.schema;
    let id = theme_block_id(&container.section_id, key);
    let closest_value = Value::object(ClosestDrop(closest.clone()));
    let block = Value::object(BlockDrop {
        id: id.clone(),
        kind: instance.kind.clone(),
        settings: settings::resolve(
            state,
            ctx,
            &schema.settings,
            &instance.settings,
            &[
                ("closest", closest_value.clone()),
                ("section", container.section.clone()),
            ],
        ),
    });

    let entered = state.enter_block(&id, key, &instance.kind)?;
    let rendered = (|| -> Result<String> {
        let mut inner = ctx.isolated()?;
        inner.set_inherited("section", container.section.clone());
        inner.set_inherited("block", block);
        inner.set_inherited("closest", closest_value);
        for (name, value) in variables {
            inner.set(name.clone(), value.clone());
        }
        inner.set(
            CONTAINER_VARIABLE,
            Value::object(Container {
                section_id: container.section_id.clone(),
                section: container.section.clone(),
                blocks: instance.blocks.clone(),
                block_order: instance.block_order.clone(),
                closest: closest.clone(),
            }),
        );
        Ok(file.template.render(&mut inner))
    })();
    state.leave_block(entered, started.elapsed());
    let content = rendered?;

    Ok(match &schema.wrapper {
        Wrapper::None => content,
        Wrapper::Tag(tag) => {
            let class = match schema.class.as_deref().filter(|class| !class.is_empty()) {
                Some(class) => format!("shopify-block {class}"),
                None => "shopify-block".to_string(),
            };
            format!("<{tag} id=\"shopify-block-{id}\" class=\"{class}\">{content}</{tag}>")
        }
    })
}
