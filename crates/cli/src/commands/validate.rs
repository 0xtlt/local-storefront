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
    match args.format {
        Format::Json => {
            let report = serde_json::json!({
                "ok": !diagnostics.has_errors(),
                "errors": diagnostics.error_count(),
                "warnings": diagnostics.warning_count(),
                "diagnostics": diagnostics.items,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
            );
        }
        Format::Text => {
            print_diagnostics(&diagnostics);
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
    Ok(if diagnostics.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
