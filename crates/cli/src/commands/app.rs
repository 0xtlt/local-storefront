//! `lsf app`: the commands of a Shopify app. `dev` serves it alone (its checkout and its customer
//! account), `init` writes the checkout scenarios to try it with, `build` builds its extensions,
//! `check` runs the scenarios for CI, and `function` runs or builds its Functions.

use std::path::Path;
use std::process::ExitCode;

use crate::commands::{app_init, function, serve};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Subcommand,
}

#[derive(clap::Subcommand)]
enum Subcommand {
    /// Serve the app alone, without a theme: its home page, its checkout (`/checkout`), where
    /// its Functions run and its checkout UI extensions show, and its customer account
    /// (`/account`), where its customer account UI extensions show. `serve --app` serves them
    /// with a theme. It takes the options of `serve`.
    Dev(serve::Args),
    /// Write checkout scenarios to the `checkouts` folder of the store data, made for the
    /// Functions and checkout UI extensions of `--app` (a generic set without it), and create the
    /// data folder with the demo store when it does not exist. Nothing that exists is
    /// overwritten without `--force`.
    Init(app_init::InitArgs),
    /// Build the extensions of the app, as `shopify app build` does: its Functions (as
    /// `lsf app function build`), and its UI extensions, of the checkout and of the customer
    /// account, that name their sources (`module`), bundled by lsf without Node
    /// (`dist/<handle>.js`).
    Build(BuildArgs),
    /// Run the Functions of every checkout scenario, as the home page checks them, for CI:
    /// exits with 1 when a scenario does not validate, a Function fails in one, or the time
    /// ran out. No HTTP request of a `.fetch` target is sent.
    Check(CheckArgs),
    /// Run or build the Shopify Functions of the app.
    Function(function::Args),
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Format {
    /// One line per problem.
    Text,
    /// `{ok, checked, total, elapsedMs, failures: [{handle, target, error, scenarios}], invalid: [{scenario, problems}]}`.
    Json,
}

#[derive(clap::Args)]
pub struct CheckArgs {
    /// The directory of a Shopify app (the one with shopify.app.toml). Defaults to the app the
    /// current folder is in.
    #[arg(long, env = "LSF_APP", value_name = "DIR")]
    app: Option<std::path::PathBuf>,

    /// Only these extensions, by handle (repeat it), as `lsf app dev --extension`.
    #[arg(long = "extension", value_name = "HANDLE")]
    extensions: Vec<String>,

    /// How to print the result.
    #[arg(long, value_enum, default_value = "text")]
    format: Format,

    /// How long the Functions may run in all, in seconds: the scenarios not run by then make it
    /// fail.
    #[arg(long, default_value_t = 300)]
    budget: u64,
}

#[derive(clap::Args)]
pub struct BuildArgs {
    /// The extensions to build, by handle. Every extension of the app when none is given.
    #[arg(value_name = "HANDLE")]
    handles: Vec<String>,

    /// The directory of a Shopify app (the one with shopify.app.toml). Defaults to the app the
    /// current folder is in.
    #[arg(long, env = "LSF_APP", value_name = "DIR")]
    app: Option<std::path::PathBuf>,
}

pub fn run(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_core::Preview],
    args: Args,
) -> Result<ExitCode, String> {
    match args.command {
        Subcommand::Dev(serve) => serve::run(theme, data, previews, serve, true),
        Subcommand::Init(init) => app_init::run(theme, data, init),
        Subcommand::Build(args) => crate::checkout::run_build(
            args.app.as_deref(),
            &args.handles,
            "lsf app build",
            crate::checkout::build,
        ),
        Subcommand::Check(args) => check(theme, data, previews, args),
        Subcommand::Function(args) => function::run(theme, data, previews, args),
    }
}

/// What `app check` says of store data without a scenario.
const NO_SCENARIO: &str =
    "no scenario to check: `lsf app init` writes a set for the app in `shopify-local/checkouts/`";

/// `lsf app check`: the check of the home page, from the command line.
fn check(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_core::Preview],
    args: CheckArgs,
) -> Result<ExitCode, String> {
    use crate::app::App;
    use crate::server::{ServeOptions, ServerState};

    let app_dir = crate::checkout::app_of(args.app.as_deref(), "lsf app check")?;
    let (shopify_app, _) = crate::checkout::open(
        &crate::checkout::AppArgs {
            app: Some(app_dir.clone()),
            extensions: args.extensions.clone(),
            build: false,
        },
        &app_dir,
    )?;
    let data = crate::app::data_with_app(theme, data, Some(&app_dir));
    let local = App::open_without_theme(theme, data.as_deref(), previews)?;
    let (state, diagnostics) = ServerState::new(
        local,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress: false,
            minify: false,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: Some(crate::checkout::CheckoutOptions {
                app_dir,
                extensions: args.extensions.clone(),
                variant: crate::checkout::Variant::Standard,
                standalone: true,
            }),
        },
    );
    if matches!(args.format, Format::Text) {
        crate::output::print_diagnostics(&diagnostics);
    }
    let store = state.loaded().store.clone();
    let report = crate::server::check_scenarios(
        &state,
        &shopify_app,
        &store,
        std::time::Duration::from_secs(args.budget),
    );
    let invalid: Vec<&lsf_core::store::CheckoutScenario> = store
        .scenarios
        .iter()
        .filter(|scenario| !scenario.problems.is_empty())
        .collect();
    let complete = report.checked == report.total;
    // No scenario, nothing checked: a check that cannot fail is not one.
    let empty = report.total == 0;
    let ok = report.failures.is_empty() && invalid.is_empty() && complete && !empty;
    match args.format {
        Format::Json => {
            let mut json = report.to_json();
            json["ok"] = serde_json::json!(ok);
            if empty {
                json["error"] = serde_json::json!(NO_SCENARIO);
            }
            json["invalid"] = serde_json::json!(invalid
                .iter()
                .map(|scenario| serde_json::json!({"scenario": scenario.name, "file": scenario.file, "problems": scenario.problems}))
                .collect::<Vec<_>>());
            println!(
                "{}",
                serde_json::to_string_pretty(&json).map_err(|error| error.to_string())?
            );
        }
        Format::Text => {
            for scenario in &invalid {
                println!("invalid  {} ({})", scenario.name, scenario.file);
                for problem in &scenario.problems {
                    println!("         {problem}");
                }
            }
            for failure in &report.failures {
                println!(
                    "fails    {} · {}: {} (in {})",
                    failure.handle,
                    failure.target,
                    failure.error,
                    failure.scenarios.join(", ")
                );
            }
            println!(
                "checked {} of {} scenarios in {:.1} s: {}",
                report.checked,
                report.total,
                report.elapsed.as_secs_f64(),
                if ok {
                    "every Function ran".to_string()
                } else if empty {
                    NO_SCENARIO.to_string()
                } else if !complete {
                    "out of time for the others (raise --budget)".to_string()
                } else {
                    format!(
                        "{} failure(s), {} invalid scenario(s)",
                        report.failures.len(),
                        invalid.len()
                    )
                }
            );
        }
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
