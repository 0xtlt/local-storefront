//! Turns stored setting values into the objects templates receive: a `color` setting becomes a
//! color, an `image_picker` an image, a `product` a product, and `{{ ... }}` dynamic sources
//! are evaluated.

use std::any::Any;
use std::borrow::Cow;
use std::sync::Arc;

use serde_json::{Map, Value as Json};
use slt_liquid::{Context, Hash, Object, Template, Value, Variable};

use super::state::RenderState;
use crate::drops::collection::CollectionDrop;
use crate::drops::color::{Color, ColorDrop};
use crate::drops::content::{ArticleDrop, BlogDrop, PageDrop};
use crate::drops::font::{Font, FontDrop};
use crate::drops::media::{ImageDrop, MediaDrop};
use crate::drops::metafield::image_from_src;
use crate::drops::navigation::LinkListDrop;
use crate::drops::product::ProductDrop;
use crate::site::Site;
use crate::store::{Media, MediaKind, MediaSource};
use crate::theme::SettingDef;
use crate::util::stable_id;

/// Evaluates dynamic sources against a context that sees the given variables.
struct Dynamic<'a> {
    parent: &'a Context,
    vars: &'a [(&'a str, Value)],
    ctx: Option<Context>,
}

impl Dynamic<'_> {
    fn ctx(&mut self) -> Option<&mut Context> {
        if self.ctx.is_none() {
            let mut ctx = self.parent.isolated().ok()?;
            for (name, value) in self.vars {
                ctx.set(*name, value.clone());
            }
            self.ctx = Some(ctx);
        }
        self.ctx.as_mut()
    }

    /// Evaluates a string that contains `{{ ... }}`. A string that is exactly one output tag
    /// yields the object itself (so a `product` setting can receive a product); anything else
    /// is rendered to text.
    fn evaluate(&mut self, source: &str) -> Value {
        let env = self.parent.shared().env.clone();
        let Some(ctx) = self.ctx() else {
            return Value::str(source);
        };
        let trimmed = source.trim();
        if let Some(inner) = trimmed
            .strip_prefix("{{")
            .and_then(|rest| rest.strip_suffix("}}"))
            && !inner.contains("{{")
            && !inner.contains("}}")
        {
            return Variable::parse(inner).evaluate(ctx).unwrap_or(Value::Nil);
        }
        match Template::parse(&env, source) {
            Ok(template) => Value::from(template.render(ctx)),
            Err(_) => Value::str(source),
        }
    }
}

/// Resolves `shopify://` links and prefixes storefront paths with the locale root.
pub fn resolve_url(site: &Site, raw: &str) -> String {
    let path = match raw.strip_prefix("shopify://") {
        Some(rest) => format!("/{rest}"),
        None => raw.to_string(),
    };
    if path.starts_with('/') && !path.starts_with("//") {
        site.request.localized(&path)
    } else {
        path
    }
}

/// The path inside `files/` a `shopify://shop_images/...` or `shopify://files/...` reference
/// points to.
fn file_reference(raw: &str) -> &str {
    raw.strip_prefix("shopify://shop_images/")
        .or_else(|| raw.strip_prefix("shopify://files/"))
        .unwrap_or(raw)
}

fn video_value(site: &Arc<Site>, raw: &str) -> Value {
    let src = file_reference(raw);
    let extension = src.rsplit('.').next().unwrap_or("mp4").to_lowercase();
    let poster = format!("{}.jpg", src.trim_end_matches(&format!(".{extension}")));
    let media = Media {
        id: stable_id("video", src),
        position: 1,
        alt: String::new(),
        preview: Some(image_from_src(site, &poster)),
        kind: MediaKind::Video {
            sources: vec![MediaSource {
                url: src.to_string(),
                mime_type: format!("video/{extension}"),
                format: extension,
                width: 1920,
                height: 1080,
            }],
            duration: 0,
        },
        aspect_ratio: 16.0 / 9.0,
    };
    MediaDrop::value(site, &media, None)
}

/// A `video_url` setting: prints as the URL and exposes the host's `id` and `type`.
struct VideoUrl {
    url: String,
    host: &'static str,
    id: String,
}

impl VideoUrl {
    fn parse(url: &str) -> Value {
        let (host, id) = if url.contains("youtu.be/") {
            ("youtube", url.rsplit('/').next().unwrap_or_default())
        } else if url.contains("youtube.com") {
            (
                "youtube",
                url.split("v=")
                    .nth(1)
                    .and_then(|rest| rest.split('&').next())
                    .unwrap_or_default(),
            )
        } else if url.contains("vimeo.com") {
            (
                "vimeo",
                url.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default(),
            )
        } else {
            return Value::str(url);
        };
        Value::object(VideoUrl {
            url: url.to_string(),
            host,
            id: id.split('?').next().unwrap_or_default().to_string(),
        })
    }
}

impl Object for VideoUrl {
    fn type_name(&self) -> &str {
        "video_url"
    }

    fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "id" => Value::from(&self.id),
            "type" => Value::str(self.host),
            _ => return None,
        })
    }

    fn to_value(&self) -> Option<Value> {
        Some(Value::from(&self.url))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A color scheme: prints as its id and exposes its colors under `settings`.
pub struct ColorScheme {
    pub id: String,
    pub settings: Value,
}

impl Object for ColorScheme {
    fn type_name(&self) -> &str {
        "color_scheme"
    }

    fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "id" => Value::from(&self.id),
            "settings" => self.settings.clone(),
            _ => return None,
        })
    }

    fn to_value(&self) -> Option<Value> {
        Some(Value::from(&self.id))
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.id)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn color_value(value: &Value) -> Value {
    match value {
        Value::Str(text) if text.trim().is_empty() => Value::Nil,
        Value::Str(text) => Color::parse(text).map_or_else(|| value.clone(), ColorDrop::value),
        other => other.clone(),
    }
}

fn handle_of(value: &Value) -> Option<&str> {
    value.as_str().filter(|handle| !handle.is_empty())
}

/// Converts one setting value according to its type.
fn convert(
    state: &RenderState,
    dynamic: &mut Dynamic<'_>,
    def: &SettingDef,
    value: Value,
) -> Value {
    let site = &state.site;
    let store = &site.store;
    // A dynamic source may already have produced the object the setting expects.
    let is_object = matches!(value, Value::Object(_));
    match def.kind.as_str() {
        "checkbox" => Value::Bool(value.is_truthy() && value.as_str() != Some("false")),
        "number" | "range" => match &value {
            Value::Str(text) => text
                .parse::<i64>()
                .map(Value::Int)
                .or_else(|_| text.parse::<f64>().map(Value::Float))
                .unwrap_or(Value::Nil),
            Value::Float(f) if f.fract() == 0.0 => Value::Int(*f as i64),
            other => other.clone(),
        },
        "color" => color_value(&value),
        "color_palette" => match &value {
            Value::Hash(map) => Value::hash(
                map.iter()
                    .map(|(key, entry)| {
                        let entry = match entry.as_str() {
                            Some(text) if text.contains("{{") => dynamic.evaluate(text),
                            _ => entry.clone(),
                        };
                        (key.clone(), color_value(&entry))
                    })
                    .collect(),
            ),
            other => other.clone(),
        },
        "font_picker" => match handle_of(&value) {
            Some(handle) => FontDrop::value(site, Font::parse(handle)),
            None if is_object => value,
            None => Value::Nil,
        },
        "image_picker" => match handle_of(&value) {
            Some(reference) => {
                ImageDrop::value(site, &image_from_src(site, file_reference(reference)))
            }
            None if is_object => value,
            None => Value::Nil,
        },
        "video" => match handle_of(&value) {
            Some(reference) => video_value(site, reference),
            None if is_object => value,
            None => Value::Nil,
        },
        "video_url" => match handle_of(&value) {
            Some(url) => VideoUrl::parse(url),
            None => Value::Nil,
        },
        "url" => match value.as_str() {
            Some(raw) if !raw.is_empty() => Value::from(resolve_url(site, raw)),
            Some(_) => Value::Nil,
            None => value,
        },
        "product" => match handle_of(&value) {
            Some(handle) => store
                .product_index(handle)
                .map_or(Value::Nil, |index| ProductDrop::value(site, index)),
            None if is_object => value,
            None => Value::Nil,
        },
        "collection" => match handle_of(&value) {
            Some(handle) => store
                .collection_index(handle)
                .map_or(Value::Nil, |index| CollectionDrop::value(site, index)),
            None if is_object => value,
            None => Value::Nil,
        },
        "page" => match handle_of(&value) {
            Some(handle) => store
                .page_index(handle)
                .map_or(Value::Nil, |index| PageDrop::value(site, index)),
            None if is_object => value,
            None => Value::Nil,
        },
        "blog" => match handle_of(&value) {
            Some(handle) => store
                .blog_index(handle)
                .map_or(Value::Nil, |index| BlogDrop::value(site, index)),
            None if is_object => value,
            None => Value::Nil,
        },
        "article" => match handle_of(&value) {
            Some(path) => path
                .split_once('/')
                .and_then(|(blog, article)| {
                    let blog_index = store.blog_index(blog)?;
                    Some(ArticleDrop::value(
                        site,
                        blog_index,
                        store.article_index(blog_index, article)?,
                    ))
                })
                .unwrap_or(Value::Nil),
            None if is_object => value,
            None => Value::Nil,
        },
        "product_list" => match &value {
            Value::Array(handles) => Value::array(
                handles
                    .iter()
                    .filter_map(|handle| store.product_index(handle.as_str()?))
                    .map(|index| ProductDrop::value(site, index))
                    .collect(),
            ),
            other if other.items().is_some() => value,
            _ => Value::array(Vec::new()),
        },
        "collection_list" => match &value {
            Value::Array(handles) => Value::array(
                handles
                    .iter()
                    .filter_map(|handle| store.collection_index(handle.as_str()?))
                    .map(|index| CollectionDrop::value(site, index))
                    .collect(),
            ),
            other if other.items().is_some() => value,
            _ => Value::array(Vec::new()),
        },
        "link_list" => match handle_of(&value) {
            Some(handle) => store
                .menu(handle)
                .map_or(Value::Nil, |menu| LinkListDrop::value(site, menu)),
            None if is_object => value,
            None => Value::Nil,
        },
        "liquid" => match value.as_str() {
            Some(source) if source.contains("{{") || source.contains("{%") => {
                let env = dynamic.parent.shared().env.clone();
                match (Template::parse(&env, source), dynamic.ctx()) {
                    (Ok(template), Some(ctx)) => Value::from(template.render(ctx)),
                    (Err(error), _) => Value::from(error.to_string()),
                    _ => value,
                }
            }
            _ => value,
        },
        "color_scheme" => match handle_of(&value) {
            Some(id) => {
                // The schemes live in the theme settings, under the color_scheme_group setting.
                let schemes = dynamic
                    .parent
                    .find_variable("settings")
                    .get("color_schemes");
                schemes
                    .items()
                    .and_then(|items| {
                        items
                            .iter()
                            .find(|scheme| scheme.get("id").as_str() == Some(id))
                            .cloned()
                    })
                    .unwrap_or_else(|| {
                        Value::object(ColorScheme {
                            id: id.to_string(),
                            settings: Value::Nil,
                        })
                    })
            }
            None => value,
        },
        "color_scheme_group" => match &value {
            Value::Hash(schemes) => Value::array(
                schemes
                    .iter()
                    .map(|(id, scheme)| {
                        let settings = match scheme.get("settings") {
                            Value::Hash(settings) => Value::hash(
                                settings
                                    .iter()
                                    .map(|(key, color)| (key.clone(), color_value(color)))
                                    .collect(),
                            ),
                            other => other,
                        };
                        Value::object(ColorScheme {
                            id: id.clone(),
                            settings,
                        })
                    })
                    .collect(),
            ),
            other => other.clone(),
        },
        _ => value,
    }
}

/// Resolves the settings of a section, a block or the theme into a hash.
///
/// Only the settings the schema declares are exposed, as on Shopify. `vars` are the variables
/// dynamic sources can refer to (`closest`, `section`, ...) in addition to the globals.
pub fn resolve(
    state: &RenderState,
    ctx: &Context,
    defs: &[SettingDef],
    values: &Map<String, Json>,
    vars: &[(&str, Value)],
) -> Value {
    let mut dynamic = Dynamic {
        parent: ctx,
        vars,
        ctx: None,
    };
    let schema_translations = state
        .site
        .theme
        .schema_translations(&state.site.request.locale);
    let mut out = Hash::with_capacity(defs.len());
    for def in defs {
        let raw: Option<&Json> = values.get(&def.id).or(def.default.as_ref());
        let value = match raw {
            None | Some(Json::Null) => Value::Nil,
            Some(Json::String(text)) => {
                if let Some(key) = text.strip_prefix("t:") {
                    // A translated default from the schema locale file.
                    schema_translations
                        .lookup(key)
                        .and_then(Json::as_str)
                        .map_or_else(|| Value::str(text), Value::str)
                } else if text.contains("{{") && def.kind != "liquid" {
                    dynamic.evaluate(text)
                } else {
                    Value::str(text)
                }
            }
            Some(other) => Value::from(other),
        };
        out.insert(def.id.clone(), convert(state, &mut dynamic, def, value));
    }
    Value::hash(out)
}

/// The theme settings: the definitions from `config/settings_schema.json`, the values from
/// `config/settings_data.json`, and any overrides from the store data.
pub fn theme_settings(state: &RenderState, ctx: &Context) -> Value {
    let theme = &state.site.theme;
    let mut defs: Vec<SettingDef> = Vec::new();
    if let Ok(Some(schema)) = theme.json("config/settings_schema.json") {
        for group in schema.as_array().into_iter().flatten() {
            defs.extend(crate::theme::schema::parse_settings(group.get("settings")));
        }
    }
    let mut values: Map<String, Json> = Map::new();
    if let Ok(Some(data)) = theme.json("config/settings_data.json") {
        let current = match data.get("current") {
            // `current` may name a preset instead of holding the values.
            Some(Json::String(preset)) => {
                data.get("presets").and_then(|presets| presets.get(preset))
            }
            other => other,
        };
        if let Some(Json::Object(current)) = current {
            values = current.clone();
        }
    }
    for (key, value) in &state.site.store.theme_settings {
        values.insert(key.clone(), value.clone());
    }

    // Settings may refer to each other (`{{ settings.color_palette.background }}`): resolve the
    // static ones first, then the dynamic ones against that first result.
    let is_dynamic = |def: &SettingDef| {
        values
            .get(&def.id)
            .or(def.default.as_ref())
            .and_then(Json::as_str)
            .is_some_and(|text| text.contains("{{"))
    };
    let (dynamic_defs, static_defs): (Vec<SettingDef>, Vec<SettingDef>) =
        defs.into_iter().partition(is_dynamic);
    let first_pass = resolve(state, ctx, &static_defs, &values, &[]);
    if dynamic_defs.is_empty() {
        return first_pass;
    }
    let second_pass = resolve(
        state,
        ctx,
        &dynamic_defs,
        &values,
        &[("settings", first_pass.clone())],
    );
    match (first_pass, second_pass) {
        (Value::Hash(first), Value::Hash(second)) => {
            let mut merged = (*first).clone();
            merged.extend(
                second
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
            Value::hash(merged)
        }
        (first, _) => first,
    }
}
