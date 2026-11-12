//! `lsf app function schema`: writes the GraphQL schema of a Function's API, at the API version
//! the Function declares, to its `schema.graphql`, as `shopify app function schema` does. From
//! the schemas lsf carries (`crates/app/schemas/functions`), without the network or an account.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lsf_app::config::version::ApiVersion;
use lsf_app::config::{App, FunctionExtension};

use crate::checkout;

/// The file the schema is written to, in the Function's folder.
pub const FILE: &str = "schema.graphql";

#[derive(clap::Args)]
pub struct SchemaArgs {
    /// The Function, by handle. Without it: the Function whose folder `--path` names, or the one
    /// the current folder is in, as `shopify app function schema` is run from a Function's
    /// folder.
    #[arg(value_name = "HANDLE")]
    handle: Option<String>,

    /// The folder of the Function, as `shopify app function schema --path`.
    #[arg(long, value_name = "DIR")]
    path: Option<PathBuf>,

    /// The directory of a Shopify app (the one with shopify.app.toml). Defaults to the app
    /// `--path` or the current folder is in.
    #[arg(long, env = "LSF_APP", value_name = "DIR")]
    app: Option<PathBuf>,

    /// Print the schema instead of writing it to the Function's `schema.graphql`.
    #[arg(long)]
    stdout: bool,
}

pub fn run(args: &SchemaArgs) -> Result<ExitCode, String> {
    let current = std::env::current_dir().map_err(|error| error.to_string())?;
    let path = args.path.as_ref().map(|path| current.join(path));
    let app_dir = match (&args.app, &path) {
        (Some(app), _) => app.clone(),
        (None, Some(path)) => checkout::app_around(path).ok_or_else(|| {
            format!(
                "--path {}: not in a Shopify app (no shopify.app.toml above it): give `--app DIR`",
                path.display()
            )
        })?,
        (None, None) => checkout::app_of(None, "lsf app function schema")?,
    };
    let app = lsf_app::config::load(&app_dir)?;
    let function = function_of(
        &app,
        args.handle.as_deref(),
        path.as_deref().unwrap_or(&current),
        "lsf app function schema",
    )?;
    let schema = schema_of(function)?;
    if args.stdout {
        print!("{schema}");
        return Ok(ExitCode::SUCCESS);
    }
    let file = function.dir.join(FILE);
    std::fs::write(&file, &schema).map_err(|error| format!("{}: {error}", file.display()))?;
    eprintln!(
        "wrote {}: the {} API, {} ({})",
        file.display(),
        api_of(function).map_or("", |api| api.name()),
        function.api_version.as_deref().unwrap_or_default(),
        function.handle
    );
    Ok(ExitCode::SUCCESS)
}

/// The Function named by `handle`, else the one whose folder `dir` is or is in. `command` is the
/// one the hints name.
pub fn function_of<'a>(
    app: &'a App,
    handle: Option<&str>,
    dir: &Path,
    command: &str,
) -> Result<&'a FunctionExtension, String> {
    let handles = || {
        app.functions
            .iter()
            .map(|function| function.handle.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(handle) = handle {
        return app.function(handle).ok_or_else(|| {
            if app.ui_extension(handle).is_some() {
                format!("`{handle}` is a UI extension, not a Function")
            } else if app.functions.is_empty() {
                format!("the app has no Function `{handle}`, nor any Function")
            } else {
                format!("the app has no Function `{handle}` (it has {})", handles())
            }
        });
    }
    let dir = lsf_core::util::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    app.functions
        .iter()
        .find(|function| {
            let own = lsf_core::util::canonicalize(&function.dir)
                .unwrap_or_else(|_| function.dir.clone());
            dir.starts_with(own)
        })
        .ok_or_else(|| {
            if app.functions.is_empty() {
                "the app has no Function".to_string()
            } else {
                format!(
                    "{} is not the folder of a Function: run it in one, give `--path` to one, or \
                     name it: `{}` (the app has {})",
                    dir.display(),
                    checkout::as_typed(&format!("{command} <handle>")),
                    handles()
                )
            }
        })
}

/// The API of a Function, from its targets.
fn api_of(function: &FunctionExtension) -> Option<lsf_app::config::version::FunctionApi> {
    function
        .targets
        .iter()
        .find_map(|target| target.spec())
        .map(|spec| spec.api)
}

/// The schema of the Function's API at its API version, without the lines lsf's copy starts
/// with (where it comes from).
pub fn schema_of(function: &FunctionExtension) -> Result<String, String> {
    let Some(api) = api_of(function) else {
        return Err(format!(
            "{}: lsf has no schema of the API of its targets ({}): it carries those of the \
             checkout's Functions (cart transform, discounts, delivery and payment \
             customizations, validations)",
            function.handle,
            function
                .targets
                .iter()
                .map(|target| target.target.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };
    let Some(version) = function.version else {
        return Err(format!(
            "{}: its api_version ({}) is not one lsf has the schemas of (supported: {})",
            function.handle,
            function.api_version.as_deref().unwrap_or("none"),
            ApiVersion::supported()
        ));
    };
    let source = version.schema_source(api);
    let body = source
        .lines()
        .skip_while(|line| line.starts_with('#') || line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!("{body}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_app() -> App {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/tests/fixtures/app");
        lsf_app::config::load(&root).expect("the fixture app")
    }

    #[test]
    fn the_schema_is_the_one_of_the_api_and_version_of_the_function() {
        let app = fixture_app();
        let payment = app.function("payment-rules").expect("payment-rules");
        let schema = schema_of(payment).unwrap();
        assert!(
            schema.starts_with("schema {\n  query: Input\n"),
            "{}",
            &schema[..80]
        );
        assert!(!schema.contains("Generated by"), "lsf's header is left out");
        assert!(schema.contains("input PaymentMethodHideOperation"));
        assert!(schema.ends_with("}\n"));
        // Each Function gets its own API's.
        for function in &app.functions {
            let Ok(schema) = schema_of(function) else {
                continue;
            };
            let api = api_of(function).unwrap();
            let lsf = function.version.unwrap().schema_source(api);
            assert!(
                lsf.ends_with(schema.trim_end()) || lsf.trim_end().ends_with(schema.trim_end()),
                "{}",
                function.handle
            );
        }
    }

    #[test]
    fn the_function_is_named_or_found_by_its_folder() {
        let app = fixture_app();
        let payment = app.function("payment-rules").unwrap();
        assert_eq!(
            function_of(&app, None, &payment.dir, "lsf app function schema")
                .unwrap()
                .handle,
            "payment-rules"
        );
        assert_eq!(
            function_of(
                &app,
                None,
                &payment.dir.join("src"),
                "lsf app function schema"
            )
            .unwrap()
            .handle,
            "payment-rules"
        );
        assert_eq!(
            function_of(
                &app,
                Some("payment-rules"),
                &app.root,
                "lsf app function schema"
            )
            .unwrap()
            .handle,
            "payment-rules"
        );
        let outside = function_of(&app, None, &app.root, "lsf app function schema").unwrap_err();
        assert!(
            outside.contains("is not the folder of a Function"),
            "{outside}"
        );
        assert!(outside.contains("payment-rules"), "{outside}");
        let unknown =
            function_of(&app, Some("nope"), &app.root, "lsf app function schema").unwrap_err();
        assert!(unknown.contains("no Function `nope`"), "{unknown}");
    }

    #[test]
    fn a_version_lsf_has_no_schema_of_is_said() {
        let mut app = fixture_app();
        let function = app
            .functions
            .iter_mut()
            .find(|function| function.handle == "payment-rules")
            .unwrap();
        function.version = None;
        function.api_version = Some("2024-01".to_string());
        let error = schema_of(function).unwrap_err();
        assert!(error.contains("2024-01"), "{error}");
        assert!(error.contains("supported: "), "{error}");
    }
}
