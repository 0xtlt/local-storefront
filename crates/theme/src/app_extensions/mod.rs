//! Theme app extensions: the app blocks and app embed blocks of the app of `--app`, rendered in
//! the theme as Shopify renders them.
//!
//! An extension is a folder of the app (`extensions/<dir>/`, `type = "theme"`) with `blocks/`,
//! `snippets/`, `assets/` and `locales/`. A JSON template names one of its app blocks by a type
//! of the form `shopify://apps/<app>/blocks/<block>/<id>`, and `config/settings_data.json` names
//! its app embed blocks the same way, under `current.blocks`.
//!
//! - [`AppExtensions`] holds the extensions of the apps served ([`Theme::set_app_extensions`]),
//!   reads their files as the theme reads its own, and finds the block a type names.
//! - [`render`] renders app blocks in sections (`{% render block %}`, `{% content_for 'blocks'
//!   %}`), app embed blocks before `</head>` and `</body>`, and the scripts and stylesheets their
//!   schemas name.
//! - [`references`] lists the app blocks a theme names, and what is wrong with them.
//! - [`validate`] checks an extension as `shopify app build` and `shopify app deploy` do.
//!
//! [`Theme::set_app_extensions`]: crate::Theme::set_app_extensions

pub mod references;
pub mod render;
pub mod validate;

use std::path::PathBuf;
use std::sync::Arc;

use lsf_core::json::parse_lenient;
use lsf_core::theme::{Revalidate, ThemeFiles};
use lsf_liquid::{Environment, Template};
use serde_json::Value as Json;

use crate::theme::schema::{self, Schema};
use crate::theme::{Cache, LiquidFile, Translations};

/// What an app gives the theme: its handle and its theme app extensions. Read from the app by
/// `lsf-cli`, which knows `shopify.app.toml`.
#[derive(Clone, Debug, Default)]
pub struct AppSource {
    /// The handle in the `shopify://apps/<handle>/...` types of its blocks.
    pub handle: String,
    /// `name` of `shopify.app.toml`, which a type may also give as a handle.
    pub name: Option<String>,
    pub extensions: Vec<ExtensionSource>,
}

/// A theme app extension on disk.
#[derive(Clone, Debug)]
pub struct ExtensionSource {
    pub handle: String,
    pub name: Option<String>,
    /// `uid` of `shopify.extension.toml`.
    pub uid: Option<String>,
    pub dir: PathBuf,
    /// How the extension is named in messages and Liquid errors: `extensions/<dir>`.
    pub label: String,
}

/// The theme app extensions of the apps served: none without `--app`.
#[derive(Default)]
pub struct AppExtensions {
    apps: Vec<ThemeApp>,
}

/// An app and its theme app extensions.
pub struct ThemeApp {
    pub handle: String,
    pub name: Option<String>,
    pub extensions: Vec<Arc<Extension>>,
}

/// A theme app extension, read as the theme is read: from the disk, cached until a file changes.
pub struct Extension {
    pub handle: String,
    pub name: Option<String>,
    pub uid: Option<String>,
    pub label: String,
    /// The handle of its app.
    pub app: String,
    files: ThemeFiles,
    env: Arc<Environment>,
    liquid: Cache<Result<Arc<LiquidFile>, lsf_liquid::Error>>,
    translations: Cache<Arc<Translations>>,
}

/// Where an app block renders, by the `target` of its schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockTarget {
    /// `section`, or `content`: an app block, in a section of a JSON template or group.
    Section,
    /// `head`: an app embed block, before `</head>`.
    Head,
    /// `compliance_head`: an app embed block, before the other ones of the head.
    ComplianceHead,
    /// `body`: an app embed block, before `</body>`.
    Body,
    /// What Shopify does not know: rendered nowhere.
    Unknown(String),
}

impl BlockTarget {
    fn of(raw: &Json) -> BlockTarget {
        match raw.get("target").and_then(Json::as_str) {
            Some("section" | "content") => BlockTarget::Section,
            Some("head") => BlockTarget::Head,
            Some("compliance_head") => BlockTarget::ComplianceHead,
            Some("body") => BlockTarget::Body,
            Some(other) => BlockTarget::Unknown(other.to_string()),
            None => BlockTarget::Unknown(String::new()),
        }
    }

    /// Whether the block is an app embed block.
    pub fn is_embed(&self) -> bool {
        matches!(
            self,
            BlockTarget::Head | BlockTarget::ComplianceHead | BlockTarget::Body
        )
    }
}

/// `enabled_on` or `disabled_on` of a schema: template types and section group types.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Restriction {
    pub templates: Option<Vec<String>>,
    pub groups: Option<Vec<String>>,
}

impl Restriction {
    fn of(raw: Option<&Json>) -> Option<Restriction> {
        let raw = raw?.as_object()?;
        let list = |key: &str| {
            raw.get(key).and_then(Json::as_array).map(|items| {
                items
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_string)
                    .collect()
            })
        };
        Some(Restriction {
            templates: list("templates"),
            groups: list("groups"),
        })
    }

    /// Whether the restriction names the place: a template type (`product`), or the type of a
    /// section group (`header`, `custom.overlay`).
    fn names(&self, place: &Place<'_>) -> bool {
        let listed = |list: &Option<Vec<String>>, value: &str| {
            list.as_ref()
                .is_some_and(|list| list.iter().any(|item| item == "*" || item == value))
        };
        match place {
            Place::Template(template) => listed(&self.templates, template),
            Place::Group(group) => listed(&self.groups, group),
        }
    }
}

/// Where a block is: in a section of a template of this type, or in a section group of this
/// type.
#[derive(Clone, Copy, Debug)]
pub enum Place<'a> {
    Template(&'a str),
    Group(&'a str),
}

impl std::fmt::Display for Place<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Place::Template(template) => write!(f, "the {template} template"),
            Place::Group(group) => write!(f, "the {group} section group"),
        }
    }
}

/// An app block or app embed block: a file of an extension's `blocks/`, parsed.
#[derive(Clone)]
pub struct AppBlock {
    pub extension: Arc<Extension>,
    /// The name of its file, without `.liquid`: what the types name.
    pub name: String,
    pub file: Arc<LiquidFile>,
    pub target: BlockTarget,
}

impl AppBlock {
    pub fn schema(&self) -> &Schema {
        &self.file.schema
    }

    fn raw(&self) -> &Json {
        &self.file.schema.raw
    }

    fn text(&self, key: &str) -> Option<&str> {
        self.raw()
            .get(key)
            .and_then(Json::as_str)
            .filter(|text| !text.is_empty())
    }

    /// `javascript`: a file of the extension's `assets/`.
    pub fn javascript(&self) -> Option<&str> {
        self.text("javascript")
    }

    /// `stylesheet`: a file of the extension's `assets/`.
    pub fn stylesheet(&self) -> Option<&str> {
        self.text("stylesheet")
    }

    /// `available_if`: Liquid that reads an app-data metafield.
    pub fn available_if(&self) -> Option<&str> {
        self.text("available_if")
    }

    /// Whether `enabled_on` and `disabled_on` let the block render at `place`. `Err` says why
    /// not.
    pub fn allowed_at(&self, place: &Place<'_>) -> Result<(), String> {
        if let Some(enabled) = Restriction::of(self.raw().get("enabled_on"))
            && !enabled.names(place)
        {
            return Err(format!("its `enabled_on` leaves out {place}"));
        }
        if let Some(disabled) = Restriction::of(self.raw().get("disabled_on"))
            && disabled.names(place)
        {
            return Err(format!("its `disabled_on` names {place}"));
        }
        Ok(())
    }

    /// The type a JSON template names it by: `shopify://apps/<app>/blocks/<block>/<id>`.
    pub fn type_name(&self) -> String {
        format!(
            "shopify://apps/{}/blocks/{}/{}",
            self.extension.app,
            self.name,
            self.extension.id()
        )
    }

    /// How messages name it: `extensions/reviews/blocks/stars.liquid`.
    pub fn path(&self) -> String {
        format!("{}/blocks/{}.liquid", self.extension.label, self.name)
    }
}

/// The parts of an app block's type: `shopify://apps/<app>/blocks/<block>/<id>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppBlockType<'a> {
    pub app: &'a str,
    pub block: &'a str,
    /// The id of the extension; Shopify CLI writes it as `uid` in `shopify.extension.toml`.
    pub id: Option<&'a str>,
}

/// The parts of a block type that names an app block, or `None` for any other type.
pub fn parse_type(kind: &str) -> Option<AppBlockType<'_>> {
    let rest = kind.strip_prefix("shopify://apps/")?;
    let mut parts = rest.split('/');
    let app = parts.next().filter(|app| !app.is_empty())?;
    if parts.next()? != "blocks" {
        return None;
    }
    let block = parts.next().filter(|block| !block.is_empty())?;
    let id = parts.next().filter(|id| !id.is_empty());
    Some(AppBlockType { app, block, id })
}

/// Whether a block type is an app block's: one of an app, or `@app`.
pub fn is_app_type(kind: &str) -> bool {
    kind.starts_with("shopify://apps/") || kind == "@app"
}

/// What a block type names.
pub enum Lookup {
    Found(AppBlock),
    /// The block file does not parse.
    Broken {
        path: String,
        error: lsf_liquid::Error,
    },
    /// The app is served, and has no block of that name.
    NoSuchBlock {
        app: String,
        block: String,
        /// The blocks it has, of the kind asked for (app blocks or app embed blocks).
        known: Vec<String>,
    },
    /// A block of an app that is not served.
    OtherApp {
        app: String,
    },
    /// Not an app block's type.
    NotAnApp,
}

impl AppExtensions {
    pub fn new(apps: Vec<AppSource>, env: Arc<Environment>, revalidate: Revalidate) -> Self {
        AppExtensions {
            apps: apps
                .into_iter()
                .map(|app| ThemeApp {
                    extensions: app
                        .extensions
                        .into_iter()
                        .map(|source| {
                            Arc::new(Extension {
                                handle: source.handle,
                                name: source.name,
                                uid: source.uid.filter(|uid| !uid.is_empty()),
                                label: source.label,
                                app: app.handle.clone(),
                                files: ThemeFiles::new(source.dir, revalidate),
                                env: env.clone(),
                                liquid: Cache::new(),
                                translations: Cache::new(),
                            })
                        })
                        .collect(),
                    handle: app.handle,
                    name: app.name,
                })
                .collect(),
        }
    }

    pub fn apps(&self) -> &[ThemeApp] {
        &self.apps
    }

    /// Whether an app with theme app extensions is served.
    pub fn is_empty(&self) -> bool {
        self.apps.iter().all(|app| app.extensions.is_empty())
    }

    pub fn extensions(&self) -> impl Iterator<Item = &Arc<Extension>> {
        self.apps.iter().flat_map(|app| app.extensions.iter())
    }

    /// The extension whose assets are served under `/cdn/extensions/<id>/`.
    pub fn extension_by_id(&self, id: &str) -> Option<&Arc<Extension>> {
        self.extensions().find(|extension| extension.id() == id)
    }

    /// Forgets what was read of the files: they are read again at the next use.
    pub fn expire(&self) {
        for extension in self.extensions() {
            extension.files.expire();
        }
    }

    /// The directories of the extensions, which live reload looks at.
    pub fn directories(&self) -> Vec<PathBuf> {
        self.extensions()
            .map(|extension| extension.files.root().to_path_buf())
            .collect()
    }

    /// Every block of every extension, in the order of the extensions and of the files.
    pub fn blocks(&self) -> Vec<Result<AppBlock, (String, lsf_liquid::Error)>> {
        self.extensions()
            .flat_map(|extension| {
                extension
                    .block_names()
                    .into_iter()
                    .filter_map(|name| extension.block(&name))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The block a type names. `embed` says which kind is asked for, for the names a
    /// `NoSuchBlock` lists.
    pub fn lookup(&self, kind: &str, embed: bool) -> Lookup {
        let Some(parsed) = parse_type(kind) else {
            return Lookup::NotAnApp;
        };
        let Some(app) = self.apps.iter().find(|app| app.matches(&parsed)) else {
            return Lookup::OtherApp {
                app: parsed.app.to_string(),
            };
        };
        // The extension the id names first, then any other that has the block: a type copied
        // from another store has the id of the extension there.
        let mut extensions: Vec<&Arc<Extension>> = app.extensions.iter().collect();
        extensions.sort_by_key(|extension| parsed.id != Some(extension.id()));
        for extension in extensions {
            match extension.block(parsed.block) {
                Some(Ok(block)) => return Lookup::Found(block),
                Some(Err((path, error))) => return Lookup::Broken { path, error },
                None => {}
            }
        }
        Lookup::NoSuchBlock {
            app: app.handle.clone(),
            block: parsed.block.to_string(),
            known: app
                .extensions
                .iter()
                .flat_map(|extension| {
                    extension
                        .block_names()
                        .into_iter()
                        .filter(|name| {
                            extension
                                .block(name)
                                .and_then(Result::ok)
                                .is_some_and(|block| block.target.is_embed() == embed)
                        })
                        .collect::<Vec<_>>()
                })
                .collect(),
        }
    }

    /// The app embed block of this name, in any extension.
    pub fn embed_named(&self, name: &str) -> Option<AppBlock> {
        self.extensions()
            .filter_map(|extension| extension.block(name).and_then(Result::ok))
            .find(|block| block.target.is_embed())
    }
}

impl ThemeApp {
    /// Whether a type names this app: by its handle, by its name as a handle, or by the id of
    /// one of its extensions.
    fn matches(&self, parsed: &AppBlockType<'_>) -> bool {
        parsed.app == self.handle
            || self
                .name
                .as_deref()
                .is_some_and(|name| handleize(name) == parsed.app)
            || parsed.id.is_some_and(|id| {
                self.extensions
                    .iter()
                    .any(|extension| extension.uid.as_deref() == Some(id))
            })
    }
}

/// A text as a Shopify handle: lowercase letters and digits, the rest turned into single dashes.
pub fn handleize(text: &str) -> String {
    let mut handle = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            handle.push(c);
        } else if !handle.is_empty() && !handle.ends_with('-') {
            handle.push('-');
        }
    }
    handle.trim_end_matches('-').to_string()
}

impl Extension {
    /// What its assets are served under: its `uid`, else its handle.
    pub fn id(&self) -> &str {
        self.uid.as_deref().unwrap_or(&self.handle)
    }

    pub fn files(&self) -> &ThemeFiles {
        &self.files
    }

    pub fn environment(&self) -> &Arc<Environment> {
        &self.env
    }

    /// The names of its blocks: the files of `blocks/`, without `.liquid`.
    pub fn block_names(&self) -> Vec<String> {
        self.files
            .list("blocks")
            .iter()
            .filter_map(|name| name.strip_suffix(".liquid"))
            .map(str::to_string)
            .collect()
    }

    /// A Liquid file of the extension (`blocks/stars.liquid`, `snippets/stars.liquid`), parsed.
    /// Its Liquid errors name it `extensions/<dir>/blocks/stars`, as the theme's name
    /// `snippets/price`. `Ok(None)` when it does not exist.
    pub fn liquid(&self, path: &str) -> Result<Option<Arc<LiquidFile>>, lsf_liquid::Error> {
        let Some(source) = self.files.read(path) else {
            return Ok(None);
        };
        self.liquid
            .get(path, source, |source| {
                let name = format!(
                    "{}/{}",
                    self.label,
                    path.strip_suffix(".liquid").unwrap_or(path)
                );
                Template::parse_named(&self.env, source, Some(&name)).map(|template| {
                    let schema = schema::extract_block(source, "schema")
                        .and_then(|body| {
                            serde_json::from_str::<Json>(&lsf_core::json::strip_comments(body)).ok()
                        })
                        .map(Schema::from_json)
                        .unwrap_or_default();
                    Arc::new(LiquidFile {
                        template: Arc::new(template),
                        schema: Arc::new(schema),
                    })
                })
            })
            .map(Some)
    }

    /// The block of this name, `None` when it has none, `Err` with the path when its file does
    /// not parse.
    pub fn block(
        self: &Arc<Self>,
        name: &str,
    ) -> Option<Result<AppBlock, (String, lsf_liquid::Error)>> {
        let path = format!("blocks/{name}.liquid");
        match self.liquid(&path) {
            Ok(Some(file)) => Some(Ok(AppBlock {
                extension: self.clone(),
                name: name.to_string(),
                target: BlockTarget::of(&file.schema.raw),
                file,
            })),
            Ok(None) => None,
            Err(error) => Some(Err((format!("{}/{path}", self.label), error))),
        }
    }

    /// The locale its `locales/<locale>.default.json` declares as the default.
    pub fn default_locale(&self) -> String {
        self.files
            .list("locales")
            .iter()
            .find_map(|name| {
                name.strip_suffix(".default.json")
                    .or_else(|| name.strip_suffix(".default.schema.json"))
            })
            .unwrap_or("en")
            .to_string()
    }

    fn translations_file(&self, path: &str) -> Option<Arc<Translations>> {
        let source = self.files.read(path)?;
        Some(self.translations.get(path, source, |source| {
            Arc::new(Translations::new(
                parse_lenient(path, source).unwrap_or(Json::Null),
            ))
        }))
    }

    /// The translations of a locale (`schema` for the `*.schema.json` files), falling back from
    /// `fr-CA` to `fr`, then to the default locale.
    fn locale_file(&self, locale: &str, schema: bool) -> Option<Arc<Translations>> {
        let default = self.default_locale();
        let language = locale.split('-').next().unwrap_or(locale);
        let kind = if schema { ".schema.json" } else { ".json" };
        for candidate in [locale, language, default.as_str()] {
            let suffix = if candidate == default { ".default" } else { "" };
            if let Some(file) =
                self.translations_file(&format!("locales/{candidate}{suffix}{kind}"))
            {
                return Some(file);
            }
        }
        None
    }

    /// The storefront translations a key of `t` is looked up in, in order: the locale's, then
    /// the default locale's.
    pub fn translations(&self, locale: &str) -> Vec<(String, Arc<Translations>)> {
        let default = self.default_locale();
        let mut found = Vec::new();
        if let Some(file) = self.locale_file(locale, false) {
            found.push((locale.to_string(), file));
        }
        if let Some(file) = self.translations_file(&format!("locales/{default}.default.json")) {
            found.push((default, file));
        }
        found
    }

    /// The translations of the `t:` keys of its schemas, in a locale.
    pub fn schema_translations(&self, locale: &str) -> Arc<Translations> {
        self.locale_file(locale, true).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_type_of_an_app_block() {
        assert_eq!(
            parse_type("shopify://apps/product-reviews/blocks/stars/3f2a"),
            Some(AppBlockType {
                app: "product-reviews",
                block: "stars",
                id: Some("3f2a")
            })
        );
        assert_eq!(
            parse_type("shopify://apps/reviews/blocks/stars").map(|kind| kind.id),
            Some(None)
        );
        assert_eq!(parse_type("shopify://apps/reviews/sections/x/1"), None);
        assert_eq!(parse_type("text"), None);
        assert!(is_app_type("@app"));
        assert!(!is_app_type("text"));
        assert_eq!(handleize("Product Reviews!"), "product-reviews");
    }

    #[test]
    fn a_restriction_names_templates_and_groups() {
        let restriction = Restriction::of(Some(&serde_json::json!({
            "templates": ["product", "metaobject/book"],
            "groups": ["footer"]
        })))
        .unwrap();
        assert!(restriction.names(&Place::Template("product")));
        assert!(!restriction.names(&Place::Template("index")));
        assert!(restriction.names(&Place::Group("footer")));
        assert!(!restriction.names(&Place::Group("header")));
        let every = Restriction::of(Some(&serde_json::json!({"templates": ["*"]}))).unwrap();
        assert!(every.names(&Place::Template("cart")));
        assert!(!every.names(&Place::Group("header")));
    }
}
