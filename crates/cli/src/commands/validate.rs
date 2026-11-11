use std::path::Path;
use std::process::ExitCode;

use lsf_core::theme::Revalidate;

use crate::app::App;
use crate::output::{count, print_diagnostics};

#[derive(Clone, Copy, clap::ValueEnum)]
enum Format {
    /// Human-readable diagnostics.
    Text,
    /// A JSON document: `{ok, errors, warnings, data, diagnostics: [{severity, code, file, path, message, hint}], app}`.
    Json,
    /// GitHub Actions annotations, one per problem (`::error file=…,line=…::message`): a
    /// workflow shows each on its line of the pull request.
    Github,
}

/// The line of the value at `pointer` in a JSON file, or of the closest value above it.
fn line_of(file: &Path, pointer: &str) -> Option<usize> {
    let source = std::fs::read_to_string(file).ok()?;
    let lines = lsf_core::json::value_lines(&lsf_core::json::strip_comments(&source));
    let mut pointer = pointer;
    loop {
        if let Some(line) = lines.get(pointer) {
            return Some(*line);
        }
        pointer = pointer.rsplit_once('/')?.0;
    }
}

/// The diagnostics of the store data as GitHub Actions annotations, their files in `data`.
fn annotations(
    diagnostics: &lsf_core::diagnostics::Diagnostics,
    data: Option<&Path>,
) -> Vec<crate::output::Annotation> {
    diagnostics
        .items
        .iter()
        .map(|item| {
            let file = data
                .map(|data| data.join(&item.file))
                .filter(|file| file.is_file());
            let mut message = item.message.clone();
            if !item.path.is_empty() {
                message = format!("at {}: {message}", item.path);
            }
            if let Some(hint) = &item.hint {
                message.push_str(&format!("\nhint: {hint}"));
            }
            crate::output::Annotation {
                level: match item.severity {
                    lsf_core::diagnostics::Severity::Error => "error",
                    _ => "warning",
                },
                line: file.as_deref().and_then(|file| line_of(file, &item.path)),
                file: file.as_ref().and_then(|_| {
                    data.map(|data| crate::output::annotation_path(data, &item.file))
                }),
                title: format!("lsf validate: {}", item.code),
                message,
            }
        })
        .collect()
}

#[derive(clap::Args)]
pub struct Args {
    /// How to print the result.
    #[arg(long, value_enum, default_value = "text")]
    format: Format,

    /// A Shopify app to check too: what its Functions and UI extensions get wrong (no build, an
    /// API version lsf does not run, an unknown target, an `owner_type`…). Defaults to the app
    /// the current folder is in, if any.
    #[arg(long, env = "LSF_APP")]
    app: Option<std::path::PathBuf>,
}

/// Whether `directory` is a theme, as `Theme::open` sees one.
fn looks_like_a_theme(directory: &Path) -> bool {
    directory.join("layout").is_dir() || directory.join("templates").is_dir()
}

pub fn run(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_theme::Preview],
    args: Args,
) -> Result<ExitCode, String> {
    // The app: `--app`, else the one the current folder is in.
    let app_dir = args.app.clone().or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|dir| crate::checkout::app_around(&dir))
    });
    // The store data, as `lsf app dev` reads it: the theme's, else the app's.
    let data = crate::app::data_with_app(theme, data, app_dir.as_deref());
    let app = if looks_like_a_theme(theme) {
        App::open(theme, data.as_deref(), Revalidate::Never, previews)?
    } else {
        // An app's store data (its checkout scenarios) has no theme: validate it alone, with
        // the store's own languages. An app without any is checked with the demo store, the
        // one its checkout runs with.
        let app = App::open_without_theme(theme, data.as_deref(), previews)?;
        if app.data_dir().is_none() && app_dir.is_none() {
            return Err(format!(
                "{} has no store data to validate: no {}/ directory, and it does not look like a \
                 Shopify theme (neither a layout/ nor a templates/ directory) or app (no \
                 shopify.app.toml). Run it in a theme or an app, or give `--data <directory>`.",
                theme.display(),
                crate::app::DEFAULT_DATA_DIRECTORY
            ));
        }
        app
    };
    let (store, mut diagnostics) = app.load_store();
    let shopify_app = match &app_dir {
        Some(dir) => Some(lsf_app::config::load(dir)?),
        None => None,
    };
    // The settings of the UI extensions, against what the extensions declare.
    for problem in shopify_app
        .iter()
        .flat_map(|shopify_app| crate::checkout::settings_problems(shopify_app, &store))
    {
        diagnostics.warning(
            "extension_settings",
            problem.file.as_deref().unwrap_or("checkout"),
            &problem.path,
            problem.message,
        );
    }
    let app_problems: Vec<&lsf_app::config::Diagnostic> = shopify_app
        .iter()
        .flat_map(|app| &app.diagnostics)
        .collect();
    let app_errors = app_problems
        .iter()
        .filter(|diagnostic| diagnostic.severity == lsf_app::config::Severity::Error)
        .count();
    let failed = diagnostics.has_errors() || app_errors > 0;
    match args.format {
        Format::Json => {
            // The counts and the diagnostics are those of the store data and of the app
            // together (an app's has the code `app`, and its extension as the file); `app` has
            // the app's own.
            let mut all =
                serde_json::to_value(&diagnostics.items).map_err(|error| error.to_string())?;
            if let Some(items) = all.as_array_mut() {
                items.extend(app_problems.iter().map(|diagnostic| {
                    serde_json::json!({
                        "severity": match diagnostic.severity {
                            lsf_app::config::Severity::Error => "error",
                            _ => "warning",
                        },
                        "code": "app",
                        "file": diagnostic.extension.as_ref().map_or_else(
                            || "shopify.app.toml".to_string(),
                            |handle| format!("extensions/{handle}"),
                        ),
                        "path": "",
                        "message": diagnostic.message,
                    })
                }));
            }
            let report = serde_json::json!({
                "ok": !failed,
                "errors": diagnostics.error_count() + app_errors,
                "warnings": diagnostics.warning_count() + app_problems.len() - app_errors,
                "data": app.describe_source(),
                "diagnostics": all,
                // What the app gets wrong, when there is one.
                "app": shopify_app.as_ref().map(|_| serde_json::json!({
                    "errors": app_errors,
                    "warnings": app_problems.len() - app_errors,
                    "diagnostics": app_problems.iter().map(|diagnostic| serde_json::json!({
                        "severity": match diagnostic.severity {
                            lsf_app::config::Severity::Error => "error",
                            _ => "warning",
                        },
                        "extension": diagnostic.extension,
                        "message": diagnostic.message,
                    })).collect::<Vec<_>>(),
                })),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
            );
        }
        Format::Github => {
            for annotation in annotations(&diagnostics, app.data_dir().as_deref()) {
                println!("{annotation}");
            }
            for diagnostic in &app_problems {
                let file = app_dir.as_deref().map(|dir| {
                    crate::output::annotation_path(
                        dir,
                        &diagnostic.extension.as_ref().map_or_else(
                            || "shopify.app.toml".to_string(),
                            |handle| format!("extensions/{handle}"),
                        ),
                    )
                });
                let level = match diagnostic.severity {
                    lsf_app::config::Severity::Error => "error",
                    _ => "warning",
                };
                println!(
                    "{}",
                    crate::output::Annotation {
                        level,
                        file,
                        line: None,
                        title: "lsf validate: app".to_string(),
                        message: diagnostic.message.clone(),
                    }
                );
            }
            eprintln!(
                "{}: {}, {}",
                app.describe_source(),
                count(
                    diagnostics.error_count() + app_errors,
                    "error",
                    "errors"
                ),
                count(
                    diagnostics.warning_count() + app_problems.len() - app_errors,
                    "warning",
                    "warnings"
                )
            );
        }
        Format::Text => {
            print_diagnostics(&diagnostics);
            for diagnostic in &app_problems {
                eprintln!("app: {diagnostic}");
            }
            if let Some(app) = &shopify_app {
                println!(
                    "app {}: {} ({}, {})",
                    app.name.as_deref().unwrap_or("(no name)"),
                    if app_errors == 0 {
                        "valid"
                    } else {
                        "not valid"
                    },
                    count(app_errors, "error", "errors"),
                    count(app_problems.len() - app_errors, "warning", "warnings")
                );
            }
            if !diagnostics.has_errors() {
                println!(
                    "{}: valid ({}, {}, {}, {}, {}, {}, {})",
                    app.describe_source(),
                    count(store.products.len(), "product", "products"),
                    count(store.collections.len(), "collection", "collections"),
                    count(store.pages.len(), "page", "pages"),
                    count(store.blogs.len(), "blog", "blogs"),
                    count(store.customers.len(), "customer", "customers"),
                    count(store.menus.len(), "menu", "menus"),
                    count(
                        store.scenarios.len(),
                        "checkout scenario",
                        "checkout scenarios"
                    )
                );
            }
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
