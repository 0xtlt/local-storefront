//! A theme on disk: Liquid files, JSON templates, settings and locales, parsed on demand and
//! cached until the files change.

mod files;
pub mod locales;
pub mod schema;
pub mod template_json;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use lsf_liquid::{Environment, Template};
use serde_json::Value as Json;

pub use files::{Revalidate, ThemeFiles};
pub use locales::Translations;
pub use schema::{Schema, SettingDef, Wrapper};
pub use template_json::{BlockInstance, Layout, SectionInstance, TemplateJson};

use crate::error::{Error, Result};
use crate::json::parse_lenient;

/// A parsed Liquid file.
pub struct LiquidFile {
    pub template: Arc<Template>,
    /// The `{% schema %}` of a section or block file.
    pub schema: Arc<Schema>,
}

struct Cached<T> {
    /// The source the value was built from; a new `Arc` means the file changed.
    source: Arc<str>,
    value: T,
}

type Cache<T> = Mutex<HashMap<String, Cached<T>>>;

pub struct Theme {
    files: ThemeFiles,
    env: Arc<Environment>,
    liquid: Cache<std::result::Result<Arc<LiquidFile>, lsf_liquid::Error>>,
    json: Cache<std::result::Result<Arc<Json>, String>>,
    translations: Cache<Arc<Translations>>,
    /// Derived values that are expensive to recompute on every request, with when they were
    /// computed.
    versions: Mutex<HashMap<&'static str, (std::time::Instant, u64)>>,
    revalidate: Revalidate,
}

/// The directories a theme is made of.
pub const THEME_DIRECTORIES: [&str; 8] = [
    "assets",
    "blocks",
    "config",
    "layout",
    "locales",
    "sections",
    "snippets",
    "templates",
];

impl Theme {
    pub fn open(
        root: impl AsRef<Path>,
        env: Arc<Environment>,
        revalidate: Revalidate,
    ) -> Result<Theme> {
        let root = root.as_ref();
        if !root.join("layout").is_dir() && !root.join("templates").is_dir() {
            return Err(Error::Theme(format!(
                "{} does not look like a Shopify theme: it has neither a layout/ nor a templates/ directory",
                root.display()
            )));
        }
        Ok(Theme {
            files: ThemeFiles::new(root, revalidate),
            env,
            liquid: Mutex::new(HashMap::new()),
            json: Mutex::new(HashMap::new()),
            translations: Mutex::new(HashMap::new()),
            versions: Mutex::new(HashMap::new()),
            revalidate,
        })
    }

    /// Memoizes a value derived from the theme's files for as long as the files are not
    /// re-checked.
    pub fn cached_version(&self, key: &'static str, compute: impl FnOnce() -> u64) -> u64 {
        if let Some((computed, value)) =
            self.versions.lock().expect("theme cache poisoned").get(key)
        {
            let fresh = match self.revalidate {
                Revalidate::Never => true,
                Revalidate::Every(interval) => {
                    computed.elapsed() < interval.max(std::time::Duration::from_secs(1))
                }
            };
            if fresh {
                return *value;
            }
        }
        let value = compute();
        self.versions
            .lock()
            .expect("theme cache poisoned")
            .insert(key, (std::time::Instant::now(), value));
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
        let mut cache = self.liquid.lock().expect("theme cache poisoned");
        if let Some(cached) = cache.get(path)
            && Arc::ptr_eq(&cached.source, &source)
        {
            return cached.value.clone().map(Some);
        }
        // The name in error messages has no extension: `snippets/price`.
        let name = path.strip_suffix(".liquid").unwrap_or(path);
        let value = Template::parse_named(&self.env, &source, Some(name)).map(|template| {
            let schema = schema::extract_block(&source, "schema")
                .and_then(|body| {
                    serde_json::from_str::<Json>(&crate::json::strip_comments(body)).ok()
                })
                .map(Schema::from_json)
                .unwrap_or_default();
            Arc::new(LiquidFile {
                template: Arc::new(template),
                schema: Arc::new(schema),
            })
        });
        cache.insert(
            path.to_string(),
            Cached {
                source,
                value: value.clone(),
            },
        );
        value.map(Some)
    }

    /// Loads a JSON file (comments allowed). `Ok(None)` means the file does not exist.
    pub fn json(&self, path: &str) -> Result<Option<Arc<Json>>> {
        let Some(source) = self.files.read(path) else {
            return Ok(None);
        };
        let mut cache = self.json.lock().expect("theme cache poisoned");
        if let Some(cached) = cache.get(path)
            && Arc::ptr_eq(&cached.source, &source)
        {
            return cached
                .value
                .clone()
                .map(Some)
                .map_err(|message| Error::Json {
                    path: path.to_string(),
                    message,
                });
        }
        let value = match parse_lenient(path, &source) {
            Ok(json) => Ok(Arc::new(json)),
            Err(Error::Json { message, .. }) => Err(message),
            Err(other) => Err(other.to_string()),
        };
        cache.insert(
            path.to_string(),
            Cached {
                source,
                value: value.clone(),
            },
        );
        value.map(Some).map_err(|message| Error::Json {
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
        let mut cache = self.translations.lock().expect("theme cache poisoned");
        if let Some(cached) = cache.get(path)
            && Arc::ptr_eq(&cached.source, &source)
        {
            return cached.value.clone();
        }
        let value = Arc::new(Translations::new(
            parse_lenient(path, &source).unwrap_or(Json::Null),
        ));
        cache.insert(
            path.to_string(),
            Cached {
                source,
                value: value.clone(),
            },
        );
        value
    }

    /// Storefront translations for a locale, falling back from `fr-CA` to `fr`.
    pub fn translations(&self, locale: &str) -> Arc<Translations> {
        let default = self.default_locale();
        let language = locale.split('-').next().unwrap_or(locale);
        for candidate in [locale, language] {
            let suffix = if candidate == default { ".default" } else { "" };
            let path = format!("locales/{candidate}{suffix}.json");
            if self.files.exists(&path) {
                return self.translations_file(&path);
            }
        }
        Arc::new(Translations::default())
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
            for file in self.files.list(directory) {
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
