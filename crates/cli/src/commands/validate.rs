use std::path::Path;
use std::process::ExitCode;

use lsf_core::theme::Revalidate;

use crate::app::App;
use crate::output::print_diagnostics;

#[derive(Clone, Copy, clap::ValueEnum)]
enum Format {
    /// Human-readable diagnostics.
    Text,
    /// A JSON document: `{ok, errors, warnings, diagnostics: [{severity, code, file, path, message, hint}]}`.
    Json,
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
    previews: &[lsf_core::Preview],
    args: Args,
) -> Result<ExitCode, String> {
    let app = if looks_like_a_theme(theme) {
        App::open(theme, data, Revalidate::Never, previews)?
    } else {
        // An app's store data (its checkout scenarios) has no theme: validate it alone, with
        // the store's own languages.
        let app = App::open_without_theme(theme, data, previews)?;
        if app.data_dir().is_none() {
            return Err(format!(
                "{} has no store data to validate: no {}/ directory, and it does not look like a \
                 Shopify theme (neither a layout/ nor a templates/ directory). Run it in a theme \
                 or an app with store data, or give `--data <directory>`.",
                theme.display(),
                crate::app::DEFAULT_DATA_DIRECTORY
            ));
        }
        app
    };
    let (store, diagnostics) = app.load_store();
    // The app: `--app`, else the one the current folder is in.
    let shopify_app = match args.app.clone().or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|dir| crate::checkout::app_around(&dir))
    }) {
        Some(dir) => Some(lsf_checkout::app::load(&dir)?),
        None => None,
    };
    let app_problems: Vec<&lsf_checkout::app::Diagnostic> = shopify_app
        .iter()
        .flat_map(|app| &app.diagnostics)
        .collect();
    let app_errors = app_problems
        .iter()
        .filter(|diagnostic| diagnostic.severity == lsf_checkout::app::Severity::Error)
        .count();
    let failed = diagnostics.has_errors() || app_errors > 0;
    match args.format {
        Format::Json => {
            let report = serde_json::json!({
                "ok": !failed,
                "errors": diagnostics.error_count(),
                "warnings": diagnostics.warning_count(),
                "diagnostics": diagnostics.items,
                // What the app gets wrong, when there is one.
                "app": shopify_app.as_ref().map(|_| serde_json::json!({
                    "errors": app_errors,
                    "warnings": app_problems.len() - app_errors,
                    "diagnostics": app_problems.iter().map(|diagnostic| serde_json::json!({
                        "severity": match diagnostic.severity {
                            lsf_checkout::app::Severity::Error => "error",
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
        Format::Text => {
            print_diagnostics(&diagnostics);
            for diagnostic in &app_problems {
                eprintln!("app: {diagnostic}");
            }
            if let Some(app) = &shopify_app {
                println!(
                    "app {}: {} ({} error(s), {} warning(s))",
                    app.name.as_deref().unwrap_or("(no name)"),
                    if app_errors == 0 {
                        "valid"
                    } else {
                        "not valid"
                    },
                    app_errors,
                    app_problems.len() - app_errors
                );
            }
            if !diagnostics.has_errors() {
                println!(
                    "{}: valid ({} products, {} collections, {} pages, {} blogs, {} customers, {} menus, {} checkout scenarios)",
                    app.describe_source(),
                    store.products.len(),
                    store.collections.len(),
                    store.pages.len(),
                    store.blogs.len(),
                    store.customers.len(),
                    store.menus.len(),
                    store.scenarios.len()
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
