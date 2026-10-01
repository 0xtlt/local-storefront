use std::path::Path;
use std::process::ExitCode;

use slt_core::theme::Revalidate;

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

pub fn run(theme: &Path, data: Option<&Path>, args: Args) -> Result<ExitCode, String> {
    let app = App::open(theme, data, Revalidate::Never)?;
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
                    "{}: valid ({} products, {} collections, {} pages, {} blogs, {} customers, {} menus)",
                    app.describe_source(),
                    store.products.len(),
                    store.collections.len(),
                    store.pages.len(),
                    store.blogs.len(),
                    store.customers.len(),
                    store.menus.len()
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
