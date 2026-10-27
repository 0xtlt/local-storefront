//! Opening a theme and its store data: shared by every command.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsf_core::diagnostics::Diagnostics;
use lsf_core::store::load::{DataSource, LoadOptions, load};
use lsf_core::theme::Revalidate;
use lsf_core::{Preview, Renderer, Store, Theme};

/// The data directory looked for inside the theme when `--data` is not given.
pub const DEFAULT_DATA_DIRECTORY: &str = "shopify-local";

pub struct App {
    pub theme: Arc<Theme>,
    pub renderer: Renderer,
    pub source: DataSource,
    /// The empty theme of an app opened without one, removed when the app is dropped.
    _scratch: Option<Scratch>,
}

/// A directory made for the process, removed with it.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Where the store data is read from: `--data`, then `<theme>/shopify-local`, then the demo.
fn data_source(theme_dir: &Path, data_dir: Option<&Path>) -> Result<DataSource, String> {
    match data_dir {
        Some(directory) => {
            if !directory.is_dir() {
                return Err(format!(
                    "the data directory {} does not exist. The `init` command creates it: `init --data {}`.",
                    directory.display(),
                    directory.display()
                ));
            }
            Ok(DataSource::Directory(directory.to_path_buf()))
        }
        None => {
            let default = theme_dir.join(DEFAULT_DATA_DIRECTORY);
            Ok(if default.is_dir() {
                DataSource::Directory(default)
            } else {
                DataSource::Demo
            })
        }
    }
}

impl App {
    /// Opens a theme with the feature previews of Shopify that `--preview` turned on: none
    /// renders it as a live store does.
    pub fn open(
        theme_dir: &Path,
        data_dir: Option<&Path>,
        revalidate: Revalidate,
        previews: &[Preview],
    ) -> Result<App, String> {
        let env = Arc::new(lsf_core::environment_with(previews));
        let theme =
            Arc::new(Theme::open(theme_dir, env, revalidate).map_err(|error| error.to_string())?);
        let source = data_source(theme_dir, data_dir)?;
        Ok(App {
            renderer: Renderer::new(theme.clone()),
            theme,
            source,
            _scratch: None,
        })
    }

    /// Opens the store data without a theme, for what does not render pages (the checkout of an
    /// app). The data is looked for in `<theme_dir>/shopify-local`, as with a theme. The theme
    /// is an empty one that lives as long as the app does.
    pub fn open_without_theme(
        theme_dir: &Path,
        data_dir: Option<&Path>,
        previews: &[Preview],
    ) -> Result<App, String> {
        let source = data_source(theme_dir, data_dir)?;
        // One folder per app: apps of one process (the tests) must not remove each other's.
        static APPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let scratch = std::env::temp_dir().join(format!(
            "lsf-no-theme-{}-{}",
            std::process::id(),
            APPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(scratch.join("layout"))
            .map_err(|error| format!("cannot create {}: {error}", scratch.display()))?;
        let scratch = Scratch(scratch);
        let env = Arc::new(lsf_core::environment_with(previews));
        let theme = Arc::new(
            Theme::open(&scratch.0, env, Revalidate::Never).map_err(|error| error.to_string())?,
        );
        Ok(App {
            renderer: Renderer::new(theme.clone()),
            theme,
            source,
            _scratch: Some(scratch),
        })
    }

    /// The directory the store data is read from, if it is on disk.
    pub fn data_dir(&self) -> Option<&PathBuf> {
        match &self.source {
            DataSource::Directory(directory) => Some(directory),
            DataSource::Demo => None,
        }
    }

    /// The theme's locales, default first, which decide the store's default language.
    pub fn load_options(&self) -> LoadOptions {
        // Without a theme there are no locales to translate to.
        if self._scratch.is_some() {
            return LoadOptions::default();
        }
        let default = self.theme.default_locale();
        let mut theme_locales = vec![default.clone()];
        theme_locales.extend(
            self.theme
                .locales()
                .into_iter()
                .filter(|locale| *locale != default),
        );
        LoadOptions { theme_locales }
    }

    pub fn load_store(&self) -> (Store, Diagnostics) {
        load(&self.source, &self.load_options())
    }

    pub fn describe_source(&self) -> String {
        match &self.source {
            DataSource::Directory(directory) => directory.display().to_string(),
            DataSource::Demo => {
                "built-in demo store (the `init` command writes an editable copy)".to_string()
            }
        }
    }
}

/// Says which feature previews the theme uses without `--preview` turning them on: its pages
/// then fail as they would on a live store, and this tells why.
pub fn note_missing_previews(theme: &Theme, previews: &[Preview]) {
    for preview in Preview::used_by(theme) {
        if !previews.contains(&preview) {
            eprintln!(
                "note: this theme uses {}, from Shopify's \"{}\" feature preview. A live store does \
                 not have them, and neither does lsf by default: run with `--preview {}` to render \
                 it as a development store with that preview does.",
                preview.summary(),
                preview.title(),
                preview.name()
            );
        }
    }
}
