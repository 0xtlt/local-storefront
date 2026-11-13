//! A theme on disk: Liquid files, JSON templates, settings and locales, parsed on demand and
//! cached until the files change.

pub mod liquid_settings;
pub mod locales;
pub mod menus;
pub mod schema;
pub mod schema_check;
pub mod template_json;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use lsf_core::error::{Error, Result};
use lsf_core::json::parse_lenient;
use lsf_core::theme::{Revalidate, ThemeFiles, ThemeIdentity};
use lsf_liquid::{Environment, Template};
use serde_json::Value as Json;

use crate::app_extensions::AppExtensions;
pub use locales::Translations;
pub use menus::MenuReference;
pub use schema::{Schema, SettingDef, Wrapper};
pub use template_json::{BlockInstance, Layout, SectionInstance, TemplateJson};

/// A parsed Liquid file.
pub struct LiquidFile {
    pub template: Arc<Template>,
    /// The `{% schema %}` of a section or block file.
    pub schema: Arc<Schema>,
    /// What Shopify refuses in that schema, which the file alone shows: a section or a theme
    /// block renders them as errors.
    pub schema_problems: Vec<schema_check::SchemaProblem>,
}

struct Cached<T> {
    /// The source the value was built from; a new `Arc` means the file changed.
    source: Arc<str>,
    value: T,
}

/// Values built from theme files, by path. Renders running at the same time all go through
/// it for every snippet and translation, so a lookup only takes the lock for reading.
pub(crate) struct Cache<T>(RwLock<HashMap<String, Cached<T>>>);

impl<T: Clone> Cache<T> {
    pub(crate) fn new() -> Self {
        Cache(RwLock::new(HashMap::new()))
    }

    fn lookup(cache: &HashMap<String, Cached<T>>, path: &str, source: &Arc<str>) -> Option<T> {
        cache
            .get(path)
            .filter(|cached| Arc::ptr_eq(&cached.source, source))
            .map(|cached| cached.value.clone())
    }

    /// The value built from `source`, built now when the file is new or has changed.
    pub(crate) fn get(&self, path: &str, source: Arc<str>, build: impl FnOnce(&str) -> T) -> T {
        if let Some(value) =
            Self::lookup(&self.0.read().expect("theme cache poisoned"), path, &source)
        {
            return value;
        }
        // Built under the write lock, so that a source is only ever built once.
        let mut cache = self.0.write().expect("theme cache poisoned");
        if let Some(value) = Self::lookup(&cache, path, &source) {
            return value;
        }
        let value = build(&source);
        cache.insert(
            path.to_string(),
            Cached {
                source,
                value: value.clone(),
            },
        );
        value
    }
}

pub struct Theme {
    files: ThemeFiles,
    env: Arc<Environment>,
    liquid: Cache<std::result::Result<Arc<LiquidFile>, lsf_liquid::Error>>,
    json: Cache<std::result::Result<Arc<Json>, String>>,
    translations: Cache<Arc<Translations>>,
    /// Derived values that are expensive to recompute on every request, with when they were
    /// computed.
    versions: Mutex<HashMap<&'static str, (std::time::Instant, u64)>>,
    /// The same for derived text.
    texts: Mutex<HashMap<&'static str, (std::time::Instant, Arc<str>)>>,
    revalidate: Revalidate,
    /// The script tags of the shop's apps that `content_for_header` loads
    /// ([`Theme::set_script_tags`]): the Admin API's, the theme's files say nothing of them.
    script_tags: RwLock<Arc<[String]>>,
    /// Which theme of the shop is served, when the Admin API published another one or renamed
    /// this one ([`Theme::set_identity`]).
    identity: RwLock<Option<Arc<ThemeIdentity>>>,
    /// The theme app extensions of the app served with the theme
    /// ([`Theme::set_app_extensions`]): their blocks render where the theme names them.
    app_extensions: RwLock<Arc<AppExtensions>>,
}

impl Theme {
    pub fn open(
        root: impl AsRef<Path>,
        env: Arc<Environment>,
        revalidate: Revalidate,
    ) -> Result<Theme> {
        let root = root.as_ref();
        if !root.exists() {
            return Err(Error::Theme(format!(
                "the theme folder {} does not exist: give `--theme` the folder of a theme (the one with layout/ and templates/)",
                root.display()
            )));
        }
        if !root.is_dir() {
            return Err(Error::Theme(format!(
                "{} is a file, not a folder: give `--theme` the folder of a theme (the one with layout/ and templates/)",
                root.display()
            )));
        }
        if !root.join("layout").is_dir() && !root.join("templates").is_dir() {
            return Err(Error::Theme(format!(
                "{} does not look like a Shopify theme: it has neither a layout/ nor a templates/ directory",
                root.display()
            )));
        }
        Ok(Theme {
            files: ThemeFiles::new(root, revalidate),
            env,
            liquid: Cache::new(),
            json: Cache::new(),
            translations: Cache::new(),
            versions: Mutex::new(HashMap::new()),
            texts: Mutex::new(HashMap::new()),
            revalidate,
            script_tags: RwLock::new(Arc::default()),
            identity: RwLock::new(None),
            app_extensions: RwLock::new(Arc::default()),
        })
    }

    /// The `src` of the script tags the storefront loads, in order: what the apps of the shop
    /// created through the Admin API (`scriptTagCreate`), which `content_for_header` loads as
    /// Shopify loads them.
    pub fn set_script_tags(&self, srcs: Vec<String>) {
        if let Ok(mut tags) = self.script_tags.write() {
            *tags = srcs.into();
        }
    }

    /// The script tags `content_for_header` loads.
    pub fn script_tags(&self) -> Arc<[String]> {
        self.script_tags
            .read()
            .map(|tags| tags.clone())
            .unwrap_or_default()
    }

    /// Says which theme of the shop is served: the Admin API's main theme. `None` for the theme
    /// directory as it is.
    pub fn set_identity(&self, identity: Option<ThemeIdentity>) {
        if let Ok(mut current) = self.identity.write() {
            *current = identity.map(Arc::new);
        }
    }

    /// The theme app extensions of the app served with the theme: their app blocks render
    /// where the templates name them, and their app embed blocks where the theme settings or
    /// the store data turn them on.
    pub fn set_app_extensions(&self, extensions: AppExtensions) {
        if let Ok(mut current) = self.app_extensions.write() {
            *current = Arc::new(extensions);
        }
    }

    /// The theme app extensions served with the theme: none without an app.
    pub fn app_extensions(&self) -> Arc<AppExtensions> {
        self.app_extensions
            .read()
            .map(|extensions| extensions.clone())
            .unwrap_or_default()
    }

    /// Which theme of the shop is served, when it is not the directory as it is.
    pub fn identity(&self) -> Option<Arc<ThemeIdentity>> {
        self.identity
            .read()
            .ok()
            .and_then(|identity| identity.clone())
    }

    /// The id of the theme served: the id of its `OnlineStoreTheme`, in `/cdn/shop/t/<id>/…`.
    pub fn id(&self) -> u64 {
        self.identity()
            .map_or(lsf_core::urls::THEME_ID, |identity| identity.id)
    }

    /// Memoizes a value derived from the theme's files for as long as the files are not
    /// re-checked.
    pub fn cached_version(&self, key: &'static str, compute: impl FnOnce() -> u64) -> u64 {
        if let Some((computed, value)) =
            self.versions.lock().expect("theme cache poisoned").get(key)
            && self.is_fresh(*computed)
        {
            return *value;
        }
        let value = compute();
        self.versions
            .lock()
            .expect("theme cache poisoned")
            .insert(key, (std::time::Instant::now(), value));
        value
    }

    /// Whether a derived value computed at `computed` can still be used.
    fn is_fresh(&self, computed: std::time::Instant) -> bool {
        match self.revalidate {
            Revalidate::Never => !self.files.overlay_changed_since(computed),
            Revalidate::Every(interval) => {
                computed.elapsed() < interval.max(std::time::Duration::from_secs(1))
                    && !self.files.expired_since(computed)
            }
        }
    }

    /// Like [`Theme::cached_version`], for text.
    pub fn cached_text(&self, key: &'static str, compute: impl FnOnce() -> String) -> Arc<str> {
        if let Some((computed, value)) = self.texts.lock().expect("theme cache poisoned").get(key)
            && self.is_fresh(*computed)
        {
            return value.clone();
        }
        let value: Arc<str> = Arc::from(compute());
        self.texts
            .lock()
            .expect("theme cache poisoned")
            .insert(key, (std::time::Instant::now(), value.clone()));
        value
    }

    pub fn files(&self) -> &ThemeFiles {
        &self.files
    }

    pub fn environment(&self) -> &Arc<Environment> {
        &self.env
    }

    /// Loads and parses a Liquid file. `Ok(None)` means the file does not exist.
    pub fn liquid(
        &self,
        path: &str,
    ) -> std::result::Result<Option<Arc<LiquidFile>>, lsf_liquid::Error> {
        let Some(source) = self.files.read(path) else {
            return Ok(None);
        };
        self.liquid
            .get(path, source, |source| {
                // The name in error messages has no extension: `snippets/price`.
                let name = path.strip_suffix(".liquid").unwrap_or(path);
                Template::parse_named(&self.env, source, Some(name)).map(|template| {
                    let schema = schema::extract_block(source, "schema")
                        .and_then(|body| {
                            serde_json::from_str::<Json>(&lsf_core::json::theme_json(body).0).ok()
                        })
                        .map(Schema::from_json)
                        .unwrap_or_default();
                    let schema_problems =
                        if path.starts_with("sections/") || path.starts_with("blocks/") {
                            schema_check::check(source)
                        } else {
                            Vec::new()
                        };
                    Arc::new(LiquidFile {
                        template: Arc::new(template),
                        schema: Arc::new(schema),
                        schema_problems,
                    })
                })
            })
            .map(Some)
    }

    /// Loads a JSON file (comments allowed). `Ok(None)` means the file does not exist.
    pub fn json(&self, path: &str) -> Result<Option<Arc<Json>>> {
        let Some(source) = self.files.read(path) else {
            return Ok(None);
        };
        self.json
            .get(path, source, |source| match parse_lenient(path, source) {
                Ok(json) => Ok(Arc::new(json)),
                Err(Error::Json { message, .. }) => Err(message),
                Err(other) => Err(other.to_string()),
            })
            .map(Some)
            .map_err(|message| Error::Json {
                path: path.to_string(),
                message,
            })
    }

    /// The schema of `sections/<kind>.liquid` or `blocks/<kind>.liquid`.
    pub fn schema(&self, path: &str) -> Arc<Schema> {
        match self.liquid(path) {
            Ok(Some(file)) => file.schema.clone(),
            _ => Arc::new(Schema::default()),
        }
    }

    /// The locale the theme declares as its default (`locales/en.default.json` → `en`).
    pub fn default_locale(&self) -> String {
        self.files
            .list("locales")
            .iter()
            .find_map(|name| name.strip_suffix(".default.json"))
            .unwrap_or("en")
            .to_string()
    }

    /// The locales the theme has storefront translations for.
    pub fn locales(&self) -> Vec<String> {
        self.files
            .list("locales")
            .iter()
            .filter(|name| !name.ends_with(".schema.json"))
            .filter_map(|name| name.strip_suffix(".json"))
            .map(|name| name.strip_suffix(".default").unwrap_or(name).to_string())
            .collect()
    }

    fn translations_file(&self, path: &str) -> Arc<Translations> {
        let Some(source) = self.files.read(path) else {
            return Arc::new(Translations::default());
        };
        self.translations.get(path, source, |source| {
            Arc::new(Translations::new(
                parse_lenient(path, source).unwrap_or(Json::Null),
            ))
        })
    }

    /// Storefront translations for a locale, falling back from `fr-CA` to `fr`.
    pub fn translations(&self, locale: &str) -> Arc<Translations> {
        match self.translations_path(locale) {
            Some(path) => self.translations_file(&path),
            None => Arc::new(Translations::default()),
        }
    }

    /// The file of the storefront translations of a locale: `locales/fr-CA.json`, else
    /// `locales/fr.json`, when the theme has one.
    pub fn translations_path(&self, locale: &str) -> Option<String> {
        let default = self.default_locale();
        let language = locale.split('-').next().unwrap_or(locale);
        [locale, language].into_iter().find_map(|candidate| {
            let suffix = if candidate == default { ".default" } else { "" };
            let path = format!("locales/{candidate}{suffix}.json");
            self.files.exists(&path).then_some(path)
        })
    }

    /// Translations for the `t:` keys used in schemas (setting defaults, names).
    pub fn schema_translations(&self, locale: &str) -> Arc<Translations> {
        let default = self.default_locale();
        for candidate in [locale, default.as_str()] {
            let suffix = if candidate == default { ".default" } else { "" };
            let path = format!("locales/{candidate}{suffix}.schema.json");
            if self.files.exists(&path) {
                return self.translations_file(&path);
            }
        }
        Arc::new(Translations::default())
    }

    /// A JSON template or a section group.
    pub fn template_json(&self, path: &str) -> Result<Option<TemplateJson>> {
        Ok(self.json(path)?.map(|json| TemplateJson::from_json(&json)))
    }

    /// The names of the templates the theme defines, e.g. `product`, `page.contact`,
    /// `customers/login`.
    pub fn template_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for (directory, prefix) in [
            ("templates", ""),
            ("templates/customers", "customers/"),
            ("templates/metaobject", "metaobject/"),
        ] {
            for file in self.files.list(directory).iter() {
                if let Some(name) = file
                    .strip_suffix(".json")
                    .or_else(|| file.strip_suffix(".liquid"))
                {
                    names.push(format!("{prefix}{name}"));
                }
            }
        }
        names
    }
}
