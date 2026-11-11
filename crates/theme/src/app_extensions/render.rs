//! Rendering app blocks and app embed blocks.
//!
//! An app block renders where the theme renders blocks: `{% render block %}` for a block of
//! `section.blocks` whose type is `@app`, and `{% content_for 'blocks' %}`. App embed blocks
//! render before `</head>` (`compliance_head`, then `head`) and before `</body>` (`body`), when
//! `config/settings_data.json` turns them on or the store data does (`app.embeds`). The Liquid of
//! an extension sees its own snippets (`render`), assets (`asset_url`) and translations (`t`),
//! and `app.metafields`.

use std::any::Any;
use std::sync::Arc;
use std::time::Instant;

use lsf_liquid::{Context, Hash, Object, PartialLoader, Result, Template, Value};
use serde_json::{Map, Value as Json};

use super::{AppBlock, AppExtensions, BlockTarget, Extension, Lookup, Place, is_app_type};
use crate::drops::metafield::MetafieldsDrop;
use crate::render::section::{BlockDrop, ClosestDrop, page_closest};
use crate::render::settings;
use crate::render::state::RenderState;
use crate::theme::Wrapper;

/// The context variable that holds the extension whose Liquid is rendering: what `asset_url`
/// and `t` read. The name cannot be written in Liquid.
pub const EXTENSION_VARIABLE: &str = "\u{0}app_extension";

/// The extension of [`EXTENSION_VARIABLE`].
pub struct ExtensionScope(pub Arc<Extension>);

impl Object for ExtensionScope {
    fn type_name(&self) -> &str {
        "app_extension"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The extension whose Liquid `ctx` renders, if any.
pub fn extension_of(ctx: &Context) -> Option<Arc<Extension>> {
    ctx.find_variable(EXTENSION_VARIABLE)
        .downcast::<ExtensionScope>()
        .map(|scope| scope.0.clone())
}

/// What `{% render 'name' %}` loads in the Liquid of an extension: its own `snippets/`.
struct ExtensionPartials(Arc<Extension>);

impl PartialLoader for ExtensionPartials {
    fn load(&self, name: &str) -> Result<Arc<Template>> {
        let path = format!("snippets/{name}.liquid");
        match self.0.liquid(&path)? {
            Some(file) => Ok(file.template.clone()),
            None => Err(lsf_liquid::Error::file_system(format!(
                "Could not find asset {}/{path}",
                self.0.label
            ))),
        }
    }
}

/// `app`: the app whose block renders. `app.metafields` are its app-data metafields.
struct AppDrop {
    metafields: Value,
}

impl Object for AppDrop {
    fn type_name(&self) -> &str {
        "app"
    }

    fn get(&self, key: &str) -> Option<Value> {
        match key {
            "metafields" => Some(self.metafields.clone()),
            _ => None,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn app_value(state: &RenderState) -> Value {
    Value::object(AppDrop {
        metafields: MetafieldsDrop::value(&state.site, &state.site.store.app.metafields),
    })
}

/// What a block of `section.blocks` that is an app block keeps for `{% render block %}`.
#[derive(Debug)]
pub struct AppBlockData {
    /// Its type: `shopify://apps/<app>/blocks/<block>/<id>`.
    pub kind: String,
    /// Its key in the JSON template.
    pub key: String,
}

/// The type of template of the page, as `enabled_on` names it: `product`,
/// `metaobject/book`, `customers/account`.
fn template_type(state: &RenderState) -> String {
    let template = &state.page.template;
    match &template.directory {
        Some(directory) => format!("{directory}/{}", template.name),
        None => template.name.clone(),
    }
}

/// Where the section `section` is: a template's, or a section group's.
fn place_of(section: &Value) -> Option<String> {
    let location = section.get("location");
    let location = location.as_str()?;
    (!matches!(location, "template" | "static" | "")).then(|| location.to_string())
}

/// Why a block type of the page renders nothing, for the terminal and `X-LSF-Reason`.
fn not_found(kind: &str, lookup: &Lookup, apps: &AppExtensions) -> Option<String> {
    Some(match lookup {
        Lookup::NoSuchBlock { app, block, known } => {
            let known = if known.is_empty() {
                "it has none".to_string()
            } else {
                format!("it has {}", known.join(", "))
            };
            format!("app block {kind} not rendered: the app {app} has no block `{block}` ({known})")
        }
        Lookup::OtherApp { app } if apps.is_empty() => format!(
            "app block {kind} not rendered: no app is served (`--app DIR` serves the app {app} and its theme app extensions)"
        ),
        Lookup::OtherApp { app } => {
            let served: Vec<&str> = apps.apps().iter().map(|app| app.handle.as_str()).collect();
            format!(
                "app block {kind} not rendered: it is a block of the app {app}, and the app served is {}",
                served.join(", ")
            )
        }
        Lookup::NotAnApp => {
            format!(
                "app block {kind} not rendered: its type is not shopify://apps/<app>/blocks/<block>/<id>"
            )
        }
        Lookup::Found(_) | Lookup::Broken { .. } => return None,
    })
}

/// The settings of an app block: its schema's, with the values of the template and the
/// translations of its extension.
pub fn block_settings(
    state: &RenderState,
    ctx: &Context,
    block: &AppBlock,
    values: &Map<String, Json>,
    vars: &[(&str, Value)],
) -> Value {
    let translations = block
        .extension
        .schema_translations(&state.site.request.locale);
    settings::resolve_translated(
        state,
        ctx,
        &block.schema().settings,
        values,
        vars,
        &translations,
    )
}

/// The block an app block type of `section.blocks` names: what `section.blocks` lists for it,
/// with its settings. `None`, and a notice, when the app has no such block.
pub fn section_block(
    state: &RenderState,
    ctx: &Context,
    key: &str,
    kind: &str,
    values: &Map<String, Json>,
    vars: &[(&str, Value)],
) -> Option<Value> {
    let apps = state.site.theme.app_extensions();
    let block = match apps.lookup(kind, false) {
        Lookup::Found(block) => block,
        // Its error is printed where `{% render block %}` would have rendered it, as the
        // template's would.
        Lookup::Broken { .. } => {
            return Some(Value::object(BlockDrop {
                id: key.to_string(),
                kind: "@app".to_string(),
                settings: Value::hash(Hash::new()),
                content: None,
                app: Some(Arc::new(AppBlockData {
                    kind: kind.to_string(),
                    key: key.to_string(),
                })),
            }));
        }
        other => {
            if let Some(notice) = not_found(kind, &other, &apps) {
                state.notice(notice);
            }
            return None;
        }
    };
    Some(Value::object(BlockDrop {
        id: key.to_string(),
        kind: "@app".to_string(),
        settings: block_settings(state, ctx, &block, values, vars),
        content: None,
        app: Some(Arc::new(AppBlockData {
            kind: kind.to_string(),
            key: key.to_string(),
        })),
    }))
}

/// One app block to render in a section.
pub struct Call<'a> {
    /// Its type: `shopify://apps/<app>/blocks/<block>/<id>`.
    pub kind: &'a str,
    /// Its id on the page: `block.id`, and the `id` of its wrapper.
    pub id: String,
    /// Its key in the JSON template.
    pub key: &'a str,
    /// The values of its settings in the template.
    pub values: &'a Map<String, Json>,
    /// The section it is in.
    pub section: Value,
    pub closest: Arc<Hash>,
    /// The variables `content_for 'blocks'` passes.
    pub variables: &'a [(String, Value)],
    /// Its settings, when `section.blocks` resolved them already.
    pub settings: Option<Value>,
}

/// Renders an app block in a section, wrapped as Shopify wraps it: `<div
/// id="shopify-block-<id>" class="shopify-block shopify-app-block">`. Nothing, and a notice that
/// says why, for a block the app does not have, an app embed block, or a block its schema keeps
/// out of the page (`enabled_on`, `disabled_on`, `available_if`).
pub fn render_block(ctx: &Context, call: Call<'_>) -> Result<String> {
    let state = RenderState::of(ctx)?;
    let apps = state.site.theme.app_extensions();
    let block = match apps.lookup(call.kind, false) {
        Lookup::Found(block) => block,
        Lookup::Broken { error, .. } => return Err(error),
        other => {
            if let Some(notice) = not_found(call.kind, &other, &apps) {
                state.notice(notice);
            }
            return Ok(String::new());
        }
    };
    if block.target != BlockTarget::Section {
        state.notice(format!(
            "app block {} not rendered in a section: its target is `{}`{}",
            block.path(),
            block
                .schema()
                .raw
                .get("target")
                .and_then(Json::as_str)
                .unwrap_or_default(),
            if block.target.is_embed() {
                ", an app embed block, which renders before </head> or </body> once turned on"
            } else {
                ", which Shopify does not know (`content`, `head`, `compliance_head` or `body`)"
            }
        ));
        return Ok(String::new());
    }
    let template = template_type(state);
    let group = place_of(&call.section);
    let place = match &group {
        Some(group) => Place::Group(group),
        None => Place::Template(&template),
    };
    if let Err(reason) = block.allowed_at(&place) {
        state.notice(format!("app block {} not rendered: {reason}", block.path()));
        return Ok(String::new());
    }
    if !available(ctx, state, &block)? {
        return Ok(String::new());
    }
    let closest_value = Value::object(ClosestDrop(call.closest.clone()));
    let settings = call.settings.unwrap_or_else(|| {
        block_settings(
            state,
            ctx,
            &block,
            call.values,
            &[
                ("closest", closest_value.clone()),
                ("section", call.section.clone()),
            ],
        )
    });
    let drop = Value::object(BlockDrop {
        id: call.id.clone(),
        kind: "@app".to_string(),
        settings,
        content: None,
        app: None,
    });
    let content = render_liquid(
        ctx,
        state,
        &block,
        drop,
        call.section,
        call.closest,
        call.variables,
        (call.key, &call.id),
    )?;
    Ok(wrap(&block, &call.id, content))
}

/// The output of a block in its wrapper: `<div id="shopify-block-<id>" class="shopify-block
/// shopify-app-block">`, in the element of its `tag`, with its `class`.
fn wrap(block: &AppBlock, id: &str, content: String) -> String {
    match &block.schema().wrapper {
        Wrapper::None => content,
        Wrapper::Tag(tag) => {
            let class = match block
                .schema()
                .class
                .as_deref()
                .filter(|class| !class.is_empty())
            {
                Some(class) => format!("shopify-block shopify-app-block {class}"),
                None => "shopify-block shopify-app-block".to_string(),
            };
            format!("<{tag} id=\"shopify-block-{id}\" class=\"{class}\">{content}</{tag}>")
        }
    }
}

/// Whether the `available_if` of a block lets it render: its Liquid, which reads an app-data
/// metafield, gives `true`. A notice says why not.
fn available(ctx: &Context, state: &RenderState, block: &AppBlock) -> Result<bool> {
    let Some(condition) = block.available_if() else {
        return Ok(true);
    };
    let template = Template::parse_named(
        &ctx.shared().env,
        condition,
        Some(&format!("{} (available_if)", block.path())),
    )?;
    let mut inner = ctx.isolated()?;
    inner.set("app", app_value(state));
    let value = template.render(&mut inner);
    if value.trim() == "true" {
        return Ok(true);
    }
    state.notice(format!(
        "app block {} not rendered: its available_if `{condition}` is `{}`, not `true` (`app.metafields` of the store data sets the app-data metafields)",
        block.path(),
        value.trim()
    ));
    Ok(false)
}

/// Renders the Liquid of a block in a context of its own: its snippets, assets and translations
/// are its extension's.
#[allow(clippy::too_many_arguments)]
fn render_liquid(
    ctx: &Context,
    state: &RenderState,
    block: &AppBlock,
    drop: Value,
    section: Value,
    closest: Arc<Hash>,
    variables: &[(String, Value)],
    (key, id): (&str, &str),
) -> Result<String> {
    let started = Instant::now();
    let _span = ctx
        .profiler()
        .map(|profiler| profiler.span(&format!("block {key}"), None));
    let kind = format!("{}/blocks/{}", block.extension.app, block.name);
    let entered = state.enter_block(id, key, &kind)?;
    let rendered = (|| -> Result<String> {
        let mut inner = ctx.isolated()?;
        inner.set_inherited("section", section);
        inner.set_inherited("block", drop);
        inner.set_inherited("closest", Value::object(ClosestDrop(closest)));
        inner.set_inherited("app", app_value(state));
        inner.set_inherited(
            EXTENSION_VARIABLE,
            Value::object(ExtensionScope(block.extension.clone())),
        );
        inner.set_partials(Arc::new(ExtensionPartials(block.extension.clone())));
        for (name, value) in variables {
            inner.set(name.clone(), value.clone());
        }
        Ok(block.file.template.render(&mut inner))
    })();
    state.leave_block(entered, started.elapsed());
    if rendered.is_ok() {
        note_assets(state, block);
    }
    rendered
}

/// Notes the `stylesheet` and `javascript` of a block that rendered.
fn note_assets(state: &RenderState, block: &AppBlock) {
    if let Some(stylesheet) = block.stylesheet() {
        state.add_app_asset(
            true,
            crate::urls::extension_asset_url(&state.site, &block.extension, stylesheet),
        );
    }
    if let Some(javascript) = block.javascript() {
        state.add_app_asset(
            false,
            crate::urls::extension_asset_url(&state.site, &block.extension, javascript),
        );
    }
}

/// An app embed block turned on for the page.
struct Embed {
    key: String,
    block: AppBlock,
    values: Map<String, Json>,
}

/// The app embed blocks turned on: those of `current.blocks` of `config/settings_data.json`
/// that are not `disabled`, then what `app.embeds` of the store data changes.
fn embeds(ctx: &Context, state: &RenderState, apps: &AppExtensions) -> Vec<Embed> {
    let theme = &state.site.theme;
    let mut found: Vec<(Embed, bool)> = Vec::new();
    let data = theme.json("config/settings_data.json").ok().flatten();
    let current = data.as_deref().and_then(|data| match data.get("current") {
        Some(Json::String(preset)) => data.get("presets").and_then(|presets| presets.get(preset)),
        other => other,
    });
    let stored = current
        .and_then(|current| current.get("blocks"))
        .and_then(Json::as_object);
    for (key, entry) in stored.into_iter().flatten() {
        let Some(kind) = entry.get("type").and_then(Json::as_str) else {
            continue;
        };
        if !is_app_type(kind) {
            continue;
        }
        let enabled = !entry
            .get("disabled")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        let values = entry
            .get("settings")
            .and_then(Json::as_object)
            .cloned()
            .unwrap_or_default();
        match apps.lookup(kind, true) {
            Lookup::Found(block) => found.push((
                Embed {
                    key: key.clone(),
                    block,
                    values,
                },
                enabled,
            )),
            Lookup::Broken { error, .. } if enabled => {
                ctx.handle_error(error, 1);
            }
            other if enabled => {
                if let Some(notice) = not_found(kind, &other, apps) {
                    state.notice(notice.replacen("app block", "app embed block", 1));
                }
            }
            _ => {}
        }
    }
    for (name, embed) in &state.site.store.app.embeds {
        if let Some((known, enabled)) = found
            .iter_mut()
            .find(|(known, _)| known.block.name == *name)
        {
            *enabled = embed.enabled;
            if let Some(settings) = &embed.settings {
                known.values = settings.clone();
            }
            continue;
        }
        if !embed.enabled {
            continue;
        }
        match apps.embed_named(name) {
            Some(block) => found.push((
                Embed {
                    key: name.clone(),
                    block,
                    values: embed.settings.clone().unwrap_or_default(),
                },
                true,
            )),
            None => {
                let known: Vec<String> = apps
                    .blocks()
                    .into_iter()
                    .filter_map(|block| block.ok())
                    .filter(|block| block.target.is_embed())
                    .map(|block| block.name)
                    .collect();
                state.notice(format!(
                    "app embed block `{name}` of `app.embeds` in the store data not rendered: the app has no app embed block of that name ({})",
                    if known.is_empty() {
                        "it has none".to_string()
                    } else {
                        format!("it has {}", known.join(", "))
                    }
                ));
            }
        }
    }
    found
        .into_iter()
        .filter(|(_, enabled)| *enabled)
        .map(|(embed, _)| embed)
        .collect()
}

/// Renders the app embed blocks turned on for the page, and loads the scripts and stylesheets of
/// the app blocks that rendered: `compliance_head` and `head` embeds, then the stylesheets and
/// scripts, before `</head>`; `body` embeds before `</body>`.
pub fn decorate(ctx: &Context, mut html: String) -> String {
    let Ok(state) = RenderState::of(ctx) else {
        return html;
    };
    let apps = state.site.theme.app_extensions();
    let (mut compliance, mut head, mut body) = (String::new(), String::new(), String::new());
    if !apps.is_empty() || !state.site.store.app.embeds.is_empty() {
        let template = template_type(state);
        let closest = page_closest(ctx);
        for embed in embeds(ctx, state, &apps) {
            let block = &embed.block;
            if let BlockTarget::Section | BlockTarget::Unknown(_) = block.target {
                state.notice(format!(
                    "app block {} not rendered as an app embed block: its target is `{}`, not `head`, `compliance_head` or `body`",
                    block.path(),
                    block.schema().raw.get("target").and_then(Json::as_str).unwrap_or_default()
                ));
                continue;
            }
            if let Err(reason) = block.allowed_at(&Place::Template(&template)) {
                state.notice(format!(
                    "app embed block {} not rendered: {reason}",
                    block.path()
                ));
                continue;
            }
            match available(ctx, state, block) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    ctx.handle_error(error, 1);
                    continue;
                }
            }
            let closest_value = Value::object(ClosestDrop(closest.clone()));
            let settings = block_settings(
                state,
                ctx,
                block,
                &embed.values,
                &[("closest", closest_value)],
            );
            let drop = Value::object(BlockDrop {
                id: embed.key.clone(),
                kind: "@app".to_string(),
                settings,
                content: None,
                app: None,
            });
            let content = match render_liquid(
                ctx,
                state,
                block,
                drop,
                Value::Nil,
                closest.clone(),
                &[],
                (&embed.key, &embed.key),
            ) {
                Ok(content) => content,
                Err(error) => ctx.handle_error(error, 1),
            };
            match block.target {
                BlockTarget::ComplianceHead => compliance.push_str(&content),
                BlockTarget::Head => head.push_str(&content),
                _ => body.push_str(&wrap(block, &embed.key, content)),
            }
        }
    }
    let mut assets = String::new();
    for (stylesheet, url) in state.app_assets() {
        if stylesheet {
            assets.push_str(&format!("<link rel=\"stylesheet\" href=\"{url}\">"));
        } else {
            assets.push_str(&format!("<script src=\"{url}\" async></script>"));
        }
    }
    // As on Shopify, embeds go before the closing tags only: a page without them (a
    // `{% layout none %}` template that writes JSON) stays as the theme wrote it.
    let in_head = format!("{compliance}{head}{assets}");
    if !in_head.is_empty()
        && let Some(index) = html.find("</head>")
    {
        html.insert_str(index, &in_head);
    }
    if !body.is_empty()
        && let Some(index) = html.rfind("</body>")
    {
        html.insert_str(index, &body);
    }
    html
}
