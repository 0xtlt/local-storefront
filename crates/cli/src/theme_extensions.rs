//! The theme app extensions of `--app`: given to the theme, which renders their blocks, and
//! described in the terminal, on the home page, by `lsf check` and by `lsf app build`.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use lsf_app::config::App;
use lsf_core::Store;
use lsf_core::theme::Revalidate;
use lsf_liquid::Environment;
use lsf_theme::Theme;
use lsf_theme::app_extensions::references::{self, Finding, Level};
use lsf_theme::app_extensions::validate::{self, Report};
use lsf_theme::app_extensions::{AppExtensions, AppSource, ExtensionSource};
use serde_json::{Value as Json, json};

use crate::output::count;

/// What the theme needs of the app: its handle and its theme app extensions.
pub fn source_of(app: &App) -> AppSource {
    AppSource {
        handle: app
            .block_handle()
            .or_else(|| {
                app.root
                    .file_name()
                    .map(|name| lsf_theme::app_extensions::handleize(&name.to_string_lossy()))
            })
            .unwrap_or_else(|| "app".to_string()),
        name: app.name.clone(),
        extensions: app
            .theme_extensions
            .iter()
            .map(|extension| ExtensionSource {
                handle: extension.handle.clone(),
                name: extension.name.clone(),
                uid: extension.uid.clone(),
                dir: extension.dir.clone(),
                label: label(app, &extension.dir),
            })
            .collect(),
    }
}

/// How an extension is named: `extensions/<dir>`.
fn label(app: &App, dir: &Path) -> String {
    dir.strip_prefix(&app.root)
        .unwrap_or(dir)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The theme app extensions of `app`, read as the theme is read.
pub fn of(app: &App, env: Arc<Environment>, revalidate: Revalidate) -> AppExtensions {
    AppExtensions::new(vec![source_of(app)], env, revalidate)
}

/// Gives the theme the theme app extensions of the app in `dir`.
pub fn attach(theme: &Theme, dir: &Path, revalidate: Revalidate) {
    if let Ok(app) = lsf_app::config::load(dir) {
        theme.set_app_extensions(of(&app, theme.environment().clone(), revalidate));
    }
}

/// What the `app:` line says of them: `1 theme extension (2 app blocks, 1 app embed block)`.
pub fn summary(apps: &AppExtensions) -> Option<String> {
    let extensions = apps.extensions().count();
    if extensions == 0 {
        return None;
    }
    let blocks: Vec<_> = apps
        .blocks()
        .into_iter()
        .filter_map(|block| block.ok())
        .collect();
    let embeds = blocks
        .iter()
        .filter(|block| block.target.is_embed())
        .count();
    Some(format!(
        "{} ({}, {})",
        count(extensions, "theme extension", "theme extensions"),
        count(blocks.len() - embeds, "app block", "app blocks"),
        count(embeds, "app embed block", "app embed blocks"),
    ))
}

/// The app embed blocks the store data turns on.
fn store_embeds(store: &Store) -> Vec<String> {
    store
        .app
        .embeds
        .iter()
        .filter(|(_, embed)| embed.enabled)
        .map(|(name, _)| name.clone())
        .collect()
}

/// Prints what is wrong with the app blocks the theme names, at start: `app: warning: ...` for
/// what renders nothing, `app: note: ...` for a block no template uses.
pub fn print_findings(theme: &Theme, store: &Store) {
    let apps = theme.app_extensions();
    if apps.is_empty() {
        return;
    }
    for finding in references::check(theme, &apps, &store_embeds(store)) {
        let place = match &finding.pointer {
            Some(pointer) => format!("{} at {pointer}", finding.file),
            None => finding.file.clone(),
        };
        let level = match finding.level {
            Level::Warning => "warning",
            Level::Info => "note",
        };
        eprintln!("app: {level}: {place}: {}", finding.message);
    }
}

/// The app blocks the theme names that render nothing, each with where it is: for the home
/// page of `--app`.
pub fn warnings(theme: &Theme, store: &Store) -> Vec<(String, String)> {
    let apps = theme.app_extensions();
    if apps.is_empty() {
        return Vec::new();
    }
    references::check(theme, &apps, &store_embeds(store))
        .into_iter()
        .filter(|finding| finding.level == Level::Warning)
        .map(|finding| {
            let place = match &finding.pointer {
                Some(pointer) => format!("{} at {pointer}", finding.file),
                None => finding.file,
            };
            (place, finding.message)
        })
        .collect()
}

/// The theme app extensions on the home page of `--app`: each with its blocks.
pub fn home(apps: &AppExtensions) -> Json {
    Json::Array(
        apps.extensions()
            .map(|extension| {
                let blocks: Vec<Json> = extension
                    .block_names()
                    .into_iter()
                    .filter_map(|name| extension.block(&name))
                    .map(|block| match block {
                        Ok(block) => json!({
                            "name": block.name,
                            "title": block.schema().name,
                            "target": block.schema().raw.get("target").cloned().unwrap_or(Json::Null),
                            "embed": block.target.is_embed(),
                            "type": block.type_name(),
                            "path": block.path(),
                        }),
                        Err((path, error)) => json!({
                            "path": path,
                            "error": error.to_string(),
                        }),
                    })
                    .collect();
                json!({
                    "handle": extension.handle,
                    "name": extension.name,
                    "path": extension.label,
                    "assets": format!("/cdn/extensions/{}/assets/", extension.id()),
                    "blocks": blocks,
                })
            })
            .collect(),
    )
}

/// The size of a file once compressed with gzip, as Shopify measures the CSS and JavaScript of
/// a block.
fn gzipped(bytes: &[u8]) -> usize {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    if encoder.write_all(bytes).is_err() {
        return bytes.len();
    }
    encoder.finish().map_or(bytes.len(), |out| out.len())
}

/// Checks the theme app extensions of `app` (those of `handles`, or all) as Shopify does when it
/// builds and deploys them, and prints what it found. Answers how many were checked and whether
/// one has an error.
pub fn validate_app(app: &App, handles: &[String]) -> (usize, bool) {
    let env = lsf_theme::environment();
    let mut checked = 0;
    let mut failed = false;
    for extension in &app.theme_extensions {
        if !handles.is_empty() && !handles.contains(&extension.handle) {
            continue;
        }
        checked += 1;
        let report: Report = validate::validate(&extension.dir, &env, &gzipped);
        let label = label(app, &extension.dir);
        for problem in &report.problems {
            let file = match (problem.file.is_empty(), problem.line) {
                (true, _) => label.clone(),
                (false, Some(line)) => format!("{label}/{}:{line}", problem.file),
                (false, None) => format!("{label}/{}", problem.file),
            };
            let level = if problem.error { "error" } else { "warning" };
            eprintln!("{level}: {file}: {}", problem.message);
        }
        if report.has_errors() {
            failed = true;
            continue;
        }
        eprintln!(
            "checked: {}: {} ({}, {}), as Shopify checks a theme app extension",
            extension.handle,
            label,
            count(report.blocks.len(), "app block", "app blocks"),
            count(report.embeds.len(), "app embed block", "app embed blocks"),
        );
    }
    (checked, failed)
}

/// What `lsf check --app` adds: the app blocks the theme names that the app does not have.
/// Without `--app`, how many app blocks the
/// theme names, which are not checked.
pub fn check_lines(theme: &Theme, app_dir: Option<&Path>) -> Result<Vec<Finding>, String> {
    let Some(dir) = app_dir else {
        let found: Vec<_> = references::references(theme)
            .into_iter()
            .filter(|reference| !reference.disabled)
            .collect();
        let Some(first) = found.first() else {
            return Ok(Vec::new());
        };
        let embeds = found.iter().filter(|reference| reference.embed).count();
        let named = match (found.len() - embeds, embeds) {
            (blocks, 0) => count(blocks, "app block", "app blocks"),
            (0, embeds) => count(embeds, "app embed block", "app embed blocks"),
            (blocks, embeds) => format!(
                "{} and {}",
                count(blocks, "app block", "app blocks"),
                count(embeds, "app embed block", "app embed blocks")
            ),
        };
        return Ok(vec![Finding {
            level: Level::Info,
            file: first.file.clone(),
            pointer: None,
            message: format!(
                "the theme names {named}, not checked: `--app DIR` checks them against the theme app extensions of the app"
            ),
        }]);
    };
    let app = lsf_app::config::load(dir).map_err(|problem| format!("--app: {problem}"))?;
    let apps = of(&app, theme.environment().clone(), Revalidate::Never);
    // `check` reads no store data: whether `app.embeds` turns an embed on is not known.
    Ok(references::check(theme, &apps, &[])
        .into_iter()
        .filter(|finding| {
            !(finding.level == Level::Info && finding.message.starts_with("app embed block"))
        })
        .collect())
}
