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
    /// Where the store data is read from. The demo store, until `pending` appears.
    source: std::sync::RwLock<DataSource>,
    /// Where the data would be when `--data` was not given and none was there yet: once a
    /// directory appears there (`lsf init`), it is read from it.
    pending: Option<PathBuf>,
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

/// Where the data of a theme without any would be: its `shopify-local/`, which `lsf init`
/// writes.
fn pending_data(theme_dir: &Path, data_dir: Option<&Path>) -> Option<PathBuf> {
    let default = theme_dir.join(DEFAULT_DATA_DIRECTORY);
    (data_dir.is_none() && !default.is_dir()).then_some(default)
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

/// The store data of a theme served with an app: `--data`, else the theme's own
/// `shopify-local/`, else the app's (`<app>/shopify-local`), else none (the demo store).
pub fn data_with_app(
    theme_dir: &Path,
    data: Option<&Path>,
    app_dir: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(data) = data {
        return Some(data.to_path_buf());
    }
    if theme_dir.join(DEFAULT_DATA_DIRECTORY).is_dir() {
        return None;
    }
    app_dir
        .map(|app| app.join(DEFAULT_DATA_DIRECTORY))
        .filter(|directory| directory.is_dir())
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
            source: std::sync::RwLock::new(source),
            pending: pending_data(theme_dir, data_dir),
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
            source: std::sync::RwLock::new(source),
            pending: pending_data(theme_dir, data_dir),
            _scratch: Some(scratch),
        })
    }

    /// Where the store data is read from now: the demo store gives way to the directory that
    /// appeared where it was looked for.
    pub fn source(&self) -> DataSource {
        if let Some(pending) = &self.pending {
            let mut source = self.source.write().expect("source lock poisoned");
            if matches!(*source, DataSource::Demo) && pending.is_dir() {
                *source = DataSource::Directory(pending.clone());
            }
        }
        self.source.read().expect("source lock poisoned").clone()
    }

    /// The directory the store data is read from, if it is on disk.
    pub fn data_dir(&self) -> Option<PathBuf> {
        match self.source() {
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
        load(&self.source(), &self.load_options())
    }

    pub fn describe_source(&self) -> String {
        match &self.source() {
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

/// What to say of the theme's `templates/customers`, the pages of classic customer accounts:
/// they are never rendered, local-storefront has Shopify's new customer accounts only. `None`
/// for a theme without them.
pub fn classic_accounts_note(theme: &Theme) -> Option<String> {
    let pages = theme
        .files()
        .list("templates/customers")
        .iter()
        .filter(|name| name.ends_with(".json") || name.ends_with(".liquid"))
        .count();
    (pages > 0).then(|| {
        format!(
            "the {pages} page(s) of templates/customers/ are for classic customer accounts, which \
             lsf does not have: they are never rendered. Every account link leads to `/account`, \
             which stands for Shopify's new customer accounts."
        )
    })
}

/// Says [`classic_accounts_note`] when there is something to say.
pub fn note_classic_accounts(theme: &Theme) {
    if let Some(note) = classic_accounts_note(theme) {
        eprintln!("note: {note}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_data_that_appears_after_the_start_is_read() {
        let theme = std::env::temp_dir().join(format!("lsf-pending-data-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&theme);
        std::fs::create_dir_all(theme.join("layout")).unwrap();
        let app = App::open_without_theme(&theme, None, &[]).unwrap();
        assert_eq!(app.data_dir(), None, "the demo store");
        // `lsf init`, while the server runs.
        std::fs::create_dir_all(theme.join(DEFAULT_DATA_DIRECTORY)).unwrap();
        assert_eq!(app.data_dir(), Some(theme.join(DEFAULT_DATA_DIRECTORY)));
        let _ = std::fs::remove_dir_all(&theme);
    }
}
