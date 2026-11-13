//! Turns stored setting values into the objects templates receive: a `color` setting becomes a
//! color, an `image_picker` an image, a `product` a product, and `{{ ... }}` dynamic sources
//! are evaluated.

use std::any::Any;
use std::borrow::Cow;
use std::sync::Arc;

use lsf_core::color::Color;
use lsf_liquid::{Context, Hash, Object, Template, Value, Variable};
use serde_json::{Map, Value as Json};

use super::state::RenderState;
use crate::drops::EmptyDrop;
use crate::drops::PaginatedList;
use crate::drops::collection::CollectionDrop;
use crate::drops::color::ColorDrop;
use crate::drops::content::{ArticleDrop, BlogDrop, PageDrop};
use crate::drops::font::{Font, FontDrop};
use crate::drops::media::{ImageDrop, MediaDrop};
use crate::drops::metafield::{MetaobjectDrop, image_from_src, video_from_src};
use crate::drops::navigation::LinkListDrop;
use crate::drops::product::ProductDrop;
use crate::site::Site;
use crate::theme::SettingDef;

/// Evaluates dynamic sources against a context that sees the given variables.
struct Dynamic<'a> {
    parent: &'a Context,
    vars: &'a [(&'a str, Value)],
    ctx: Option<Context>,
    /// The `color_scheme_group` of the settings being resolved, when they have one: the theme
    /// settings, whose `color_scheme` settings cannot read `settings.color_schemes` yet.
    schemes: Option<Value>,
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

/// Where the value of a `url` setting comes from, which decides whether it gets the locale prefix.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UrlOrigin {
    /// Saved in the settings: picked in the theme editor (`shopify://…`) or entered as text.
    Saved,
    /// The `default` of the schema, `/collections` or `/collections/all`.
    Default,
    /// A dynamic source (`{{ page.url }}`).
    Dynamic,
}

/// The URL a `url` setting prints, as the `url` object's `url` gives it
/// (<https://shopify.dev/docs/api/liquid/objects/url>): "URLs that point to store resources,
/// the home page, and the collection pages include the path prefix of the current locale",
/// "relative paths and anchors are returned exactly as they were entered". So a resource picked
/// in the theme editor (`shopify://…`), the home page (`/`, a `home_link`) and the `default`
/// (the docs accept only `/collections` and `/collections/all`, the collection pages) are
/// prefixed; a path entered as text (a `relative_link` "even if the path matches a resource"),
/// a path from a dynamic source, an anchor and an absolute URL are printed as they are.
fn resolve_url(site: &Site, raw: &str, origin: UrlOrigin) -> String {
    let path = match raw.strip_prefix("shopify://") {
        // The page of a metaobject entry: `shopify://metaobjects/<type>/<handle>`.
        Some(rest) if rest.starts_with("metaobjects/") => {
            format!("/pages/{}", &rest["metaobjects/".len()..])
        }
        Some(rest) => format!("/{rest}"),
        None if origin == UrlOrigin::Default || (origin == UrlOrigin::Saved && raw == "/") => {
            raw.to_string()
        }
        None => return raw.to_string(),
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
    MediaDrop::value(site, &video_from_src(site, file_reference(raw)), None)
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

/// A `color_scheme_group` setting (`settings.color_schemes`): its schemes in a `for` loop, and
/// one of them by id, `settings.color_schemes['scheme-1']`.
pub struct ColorSchemeGroup(Arc<Vec<Value>>);

impl ColorSchemeGroup {
    fn value(schemes: Vec<Value>) -> Value {
        Value::object(ColorSchemeGroup(Arc::new(schemes)))
    }
}

impl Object for ColorSchemeGroup {
    fn type_name(&self) -> &str {
        "color_scheme_group"
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.0
            .iter()
            .find(|scheme| scheme.get("id").as_str() == Some(key))
            .cloned()
    }

    fn index(&self, index: i64) -> Option<Value> {
        let len = self.0.len() as i64;
        let index = if index < 0 { index + len } else { index };
        (index >= 0 && index < len).then(|| self.0[index as usize].clone())
    }

    fn items(&self) -> Option<Arc<Vec<Value>>> {
        Some(self.0.clone())
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Owned(self.0.iter().map(|scheme| scheme.to_str()).collect())
    }

    fn to_json(&self) -> Json {
        Json::Array(self.0.iter().map(Value::to_json).collect())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The value of a `product`, `collection`, `page`, `blog`, `article` or `link_list` setting that
/// names a resource: the resource itself, which prints its handle, as Shopify's does for the
/// themes that still read the handle a setting held before it held the object. `size` is the
/// length of that handle. Everything else is the resource's: its properties, its `json`, its
/// comparisons, and the drop a filter or a tag downcasts it to.
struct ResourceSetting {
    resource: Arc<dyn Object>,
    handle: String,
}

impl ResourceSetting {
    fn value(resource: Value) -> Value {
        let Value::Object(object) = &resource else {
            return resource;
        };
        let handle = object
            .get("handle")
            .map(|handle| handle.to_str().into_owned());
        match handle {
            Some(handle) => Value::object(ResourceSetting {
                resource: object.clone(),
                handle,
            }),
            None => resource,
        }
    }
}

impl Object for ResourceSetting {
    fn type_name(&self) -> &str {
        self.resource.type_name()
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.resource.get(key)
    }

    fn index(&self, index: i64) -> Option<Value> {
        self.resource.index(index)
    }

    fn items(&self) -> Option<Arc<Vec<Value>>> {
        self.resource.items()
    }

    fn items_until(&self, to: usize) -> Option<Arc<Vec<Value>>> {
        self.resource.items_until(to)
    }

    fn size(&self) -> Option<usize> {
        Some(self.handle.chars().count())
    }

    fn to_value(&self) -> Option<Value> {
        self.resource.to_value()
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.handle)
    }

    fn to_json(&self) -> Json {
        self.resource.to_json()
    }

    fn identity(&self) -> Option<String> {
        self.resource.identity()
    }

    fn equals(&self, other: &Value) -> Option<bool> {
        self.resource.equals(other)
    }

    /// The resource's own drop, so that `downcast::<ProductDrop>()` finds it.
    fn as_any(&self) -> &dyn Any {
        self.resource.as_any()
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
    origin: UrlOrigin,
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
        // A cleared color is stored as `""`, and is `""`, which is `blank`.
        "color" => match &value {
            Value::Str(text) if text.trim().is_empty() => Value::empty_string(),
            _ => color_value(&value),
        },
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
            Some(raw) if !raw.is_empty() => Value::from(resolve_url(site, raw, origin)),
            Some(_) => Value::Nil,
            None => value,
        },
        "product" => match handle_of(&value) {
            Some(handle) => store.product_index(handle).map_or_else(
                || EmptyDrop::setting(EmptyDrop::NONE, handle),
                |index| ResourceSetting::value(ProductDrop::value(site, index)),
            ),
            None if is_object => value,
            None => Value::Nil,
        },
        "collection" => match handle_of(&value) {
            Some(handle) => store.collection_index(handle).map_or_else(
                || EmptyDrop::setting(EmptyDrop::COLLECTION, handle),
                |index| ResourceSetting::value(CollectionDrop::value(site, index)),
            ),
            None if is_object => value,
            None => Value::Nil,
        },
        "page" => match handle_of(&value) {
            Some(handle) => store.page_index(handle).map_or_else(
                || EmptyDrop::setting(EmptyDrop::NONE, handle),
                |index| ResourceSetting::value(PageDrop::value(site, index)),
            ),
            None if is_object => value,
            None => Value::Nil,
        },
        "blog" => match handle_of(&value) {
            Some(handle) => store.blog_index(handle).map_or_else(
                || EmptyDrop::setting(EmptyDrop::BLOG, handle),
                |index| ResourceSetting::value(BlogDrop::value(site, index)),
            ),
            None if is_object => value,
            None => Value::Nil,
        },
        "article" => match handle_of(&value) {
            Some(path) => path
                .split_once('/')
                .and_then(|(blog, article)| {
                    let blog_index = store.blog_index(blog)?;
                    Some(ResourceSetting::value(ArticleDrop::value(
                        site,
                        blog_index,
                        store.article_index(blog_index, article)?,
                    )))
                })
                .unwrap_or_else(|| EmptyDrop::setting(EmptyDrop::NONE, path)),
            None if is_object => value,
            None => Value::Nil,
        },
        // Shopify stores the entry's id; themes also write its handle, alone or with its type.
        "metaobject" => match handle_of(&value) {
            Some(reference) => store
                .metaobject_by_reference(reference, def.metaobject_type.as_deref())
                .map_or(Value::Nil, |entry| MetaobjectDrop::value(site, entry)),
            None if is_object => value,
            None => Value::Nil,
        },
        "metaobject_list" => match value.items() {
            // A dynamic source may give entries already; stored references are looked up.
            Some(references) => Value::array(
                references
                    .iter()
                    .filter_map(|reference| {
                        if reference.downcast::<MetaobjectDrop>().is_some() {
                            return Some(reference.clone());
                        }
                        store
                            .metaobject_by_reference(
                                reference.as_str()?,
                                def.metaobject_type.as_deref(),
                            )
                            .map(|entry| MetaobjectDrop::value(site, entry))
                    })
                    .collect(),
            ),
            None => Value::array(Vec::new()),
        },
        // Shopify's `ProductListDrop`, empty when nothing is selected. Stored handles are
        // looked up; a dynamic source (`{{ product.metafields.custom.related.value }}`) gives
        // the products themselves, which are kept in their order.
        "product_list" => PaginatedList::list_drop(
            "product_list",
            value
                .items()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            if item.downcast::<ProductDrop>().is_some() {
                                return Some(item.clone());
                            }
                            let index = store.product_index(item.as_str()?)?;
                            Some(ProductDrop::value(site, index))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
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
            Some(handle) => store.menu(handle).map_or_else(
                || EmptyDrop::setting(EmptyDrop::LINK_LIST, handle),
                |menu| ResourceSetting::value(LinkListDrop::value(site, menu)),
            ),
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
                let schemes = dynamic.schemes.clone().unwrap_or_else(|| {
                    dynamic
                        .parent
                        .find_variable("settings")
                        .get("color_schemes")
                });
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
            Value::Hash(schemes) => ColorSchemeGroup::value(
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

/// What a setting with neither a value nor a `default` is, as Shopify's input settings reference
/// says: the first option of a `select` or a `radio`, `left` for a `text_alignment`, an empty
/// string for the settings that hold text or a color, and nil for the others (`number`, `url`,
/// `image_picker`, a resource, ...).
fn unset(def: &SettingDef) -> Value {
    match def.kind.as_str() {
        "select" | "radio" => def.first_option.as_deref().map_or(Value::Nil, Value::str),
        "text_alignment" => Value::str("left"),
        "text" | "textarea" | "richtext" | "inline_richtext" | "html" | "color"
        | "color_background" => Value::empty_string(),
        _ => Value::Nil,
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
    resolve_translated(
        state,
        ctx,
        defs,
        values,
        vars,
        state.site.schema_translations(),
    )
}

/// [`resolve`], with the `t:` defaults looked up in `schema_translations`: those of a theme app
/// extension for its blocks.
pub fn resolve_translated(
    state: &RenderState,
    ctx: &Context,
    defs: &[SettingDef],
    values: &Map<String, Json>,
    vars: &[(&str, Value)],
    schema_translations: &crate::theme::Translations,
) -> Value {
    let mut dynamic = Dynamic {
        parent: ctx,
        vars,
        ctx: None,
        schemes: None,
    };
    // The color schemes first, for the `color_scheme` settings beside them, whatever their order.
    let group = defs
        .iter()
        .find(|def| def.kind == "color_scheme_group")
        .map(|def| {
            let value = values
                .get(&def.id)
                .or(def.default.as_ref())
                .map_or(Value::Nil, Value::from);
            let origin = url_origin(def, values);
            (
                def.id.as_str(),
                convert(state, &mut dynamic, def, value, origin),
            )
        });
    dynamic.schemes = group.as_ref().map(|(_, schemes)| schemes.clone());
    let mut out = Hash::with_capacity(defs.len());
    for def in defs {
        if let Some((id, schemes)) = &group
            && def.id == *id
        {
            out.insert(def.id.clone(), schemes.clone());
            continue;
        }
        let raw: Option<&Json> = values.get(&def.id).or(def.default.as_ref());
        let value = match raw {
            None | Some(Json::Null) => unset(def),
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
        out.insert(
            def.id.clone(),
            convert(state, &mut dynamic, def, value, url_origin(def, values)),
        );
    }
    Value::hash(out)
}

/// Where the value of a setting comes from, for a `url` setting: see [`resolve_url`].
fn url_origin(def: &SettingDef, values: &Map<String, Json>) -> UrlOrigin {
    match values.get(&def.id).or(def.default.as_ref()) {
        Some(Json::String(text)) if text.contains("{{") => UrlOrigin::Dynamic,
        _ if values.contains_key(&def.id) => UrlOrigin::Saved,
        _ => UrlOrigin::Default,
    }
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
