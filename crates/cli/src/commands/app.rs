//! `lsf app`: the commands of a Shopify app. `dev` serves it alone (its checkout and its customer
//! account), `init` writes the checkout scenarios to try it with, `build` builds its extensions,
//! `check` runs the scenarios for CI, `function` runs or builds its Functions, and `webhook
//! trigger` sends one of its webhooks.

use std::path::Path;
use std::process::ExitCode;

use crate::commands::{app_init, function, serve, webhook};
use crate::output::count;

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
    /// Run the Functions of every checkout scenario, as the home page checks them, for CI, and
    /// check what the checkout of each must look like (its `expect`): exits with 1 when a
    /// scenario does not validate, a Function fails in one, an expectation is not met, or the
    /// time ran out. No HTTP request of a `.fetch` target is sent.
    Check(CheckArgs),
    /// Run or build the Shopify Functions of the app.
    Function(function::Args),
    /// Send the webhooks of the app to its backend.
    Webhook(webhook::Args),
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Format {
    /// One line per problem.
    Text,
    /// `{ok, state, checked, total, elapsedMs, failures: [{handle, target, error, logs, scenarios}], invalid: [{scenario, file, problems}], expectations: {checked, unchecked: [scenario], unmet: [{scenario, file, step, assertion, expected, actual}]}, ranInNoScenario: [handle]}`, and `error` when there is no scenario. `state` is `done`, or `partial` when the time ran out. `logs` is what the Function logged in the first scenario it failed in. `expectations.checked` counts the expectations of the scenarios' `expect`, met or not; `unchecked` names the scenarios whose `expect` was not checked, as `--extension` leaves out a Function they run; `assertion` is the list and the rule (`express_methods.excludes`) or the amount (`totals.total`), `expected` what the scenario says, `actual` what the checkout has. `ranInNoScenario` is filled only when every scenario ran.
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

    /// Build the extensions first, as `lsf app build` does: for a clone whose `dist/` is not
    /// committed.
    #[arg(long)]
    build: bool,
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
    previews: &[lsf_theme::Preview],
    args: Args,
    given: &serve::Given,
) -> Result<ExitCode, String> {
    match args.command {
        Subcommand::Dev(serve) => serve::run(theme, data, previews, serve, given, true),
        Subcommand::Init(init) => app_init::run(theme, data, init),
        Subcommand::Build(args) => crate::checkout::run_build(
            args.app.as_deref(),
            &args.handles,
            "lsf app build",
            crate::checkout::build,
        ),
        Subcommand::Check(args) => check(theme, data, previews, args),
        Subcommand::Function(args) => function::run(theme, data, previews, args),
        Subcommand::Webhook(args) => webhook::run(theme, data, previews, args),
    }
}

/// Whether a Function is a discount: one runs only for a discount of the checkout that names it.
fn is_discount(function: &lsf_app::config::FunctionExtension) -> bool {
    use lsf_app::config::TargetKind;
    function.targets.iter().any(|target| {
        matches!(
            target.kind,
            TargetKind::LinesDiscounts
                | TargetKind::ProductDiscount
                | TargetKind::OrderDiscount
                | TargetKind::DeliveryDiscounts
                | TargetKind::ShippingDiscount
        )
    })
}

/// What `app check` says of store data without a scenario.
static NO_SCENARIO: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "no scenario to check: `{} app init` writes a set for the app in `shopify-local/checkouts/`",
        lsf_core::util::command()
    )
});

/// `lsf app check`: the check of the home page, from the command line.
fn check(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_theme::Preview],
    args: CheckArgs,
) -> Result<ExitCode, String> {
    use crate::app::App;
    use crate::server::{ServeOptions, ServerState};

    let app_dir = crate::checkout::app_of(args.app.as_deref(), "lsf app check")?;
    let (shopify_app, selection) = crate::checkout::open(
        &crate::checkout::AppArgs {
            app: Some(app_dir.clone()),
            extensions: args.extensions.clone(),
            build: args.build,
        },
        &app_dir,
    )?;
    let data = crate::app::data_with_app(theme, data, Some(&app_dir));
    let local = App::open_without_theme(theme, data.as_deref(), previews)?;
    let (state, diagnostics) = ServerState::new(
        local,
        ServeOptions {
            polaris: Default::default(),
            admin: None,
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
                no_functions: false,
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
    let ok = report.failures.is_empty()
        && invalid.is_empty()
        && report.unmet.is_empty()
        && complete
        && !empty;
    // The Functions no scenario ran: nothing checks them. A warning, not a failure: a Function
    // may be meant for a case the scenarios leave aside. Only known once every scenario ran.
    let ran_nowhere: Vec<&lsf_app::config::FunctionExtension> = if complete && !empty {
        selection
            .functions(&shopify_app)
            .filter(|function| !report.ran.contains(&function.handle))
            .collect()
    } else {
        Vec::new()
    };
    match args.format {
        Format::Json => {
            let mut json = report.to_json();
            json["ok"] = serde_json::json!(ok);
            if empty {
                json["error"] = serde_json::json!(*NO_SCENARIO);
            }
            json["invalid"] = serde_json::json!(invalid
                .iter()
                .map(|scenario| serde_json::json!({"scenario": scenario.name, "file": scenario.file, "problems": scenario.problems}))
                .collect::<Vec<_>>());
            json["expectations"] = serde_json::json!({
                "checked": report.expectations,
                "unchecked": report.unchecked,
                "unmet": report.unmet.iter().map(|unmet| serde_json::json!({
                    "scenario": unmet.scenario,
                    "file": unmet.file,
                    "step": unmet.step,
                    "assertion": unmet.unmet.assertion,
                    "expected": unmet.unmet.expected,
                    "actual": unmet.unmet.actual,
                })).collect::<Vec<_>>(),
            });
            json["ranInNoScenario"] = serde_json::json!(
                ran_nowhere
                    .iter()
                    .map(|function| &function.handle)
                    .collect::<Vec<_>>()
            );
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
            for function in &ran_nowhere {
                println!(
                    "warning  {}: ran in no scenario, so nothing checks it{}",
                    function.handle,
                    if is_discount(function) {
                        " (a discount Function runs only for a discount that names it: add one \
                         to `checkout.discounts` of the store data or of a scenario)"
                    } else if function.owner_type().is_some() {
                        " (it runs where the shop has an enabled owner of it, as on Shopify, or \
                         where a scenario names it: add it to the `extensions` of one)"
                    } else {
                        ""
                    }
                );
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
            for unmet in &report.unmet {
                print_unmet(unmet);
            }
            let expectations = count(report.expectations, "expectation", "expectations");
            let mut said = if empty {
                NO_SCENARIO.to_string()
            } else if !complete {
                "out of time for the others (raise --budget)".to_string()
            } else if !ok {
                let mut said = format!(
                    "{}, {}",
                    count(report.failures.len(), "failure", "failures"),
                    count(invalid.len(), "invalid scenario", "invalid scenarios")
                );
                if report.expectations > 0 {
                    said.push_str(&format!(", {} of {expectations} unmet", report.unmet.len()));
                }
                said
            } else if ran_nowhere.is_empty() {
                "every Function ran".to_string()
            } else {
                // Not "every Function ran": some did not, as the warnings above say.
                "no Function failed".to_string()
            };
            if ok && report.expectations > 0 {
                said.push_str(&format!(", {expectations} met"));
            }
            if !report.unchecked.is_empty() {
                let (scenarios, they) = match report.unchecked.len() {
                    1 => ("1 scenario".to_string(), "it runs"),
                    n => (format!("{n} scenarios"), "they run"),
                };
                said.push_str(&format!(
                    ", the `expect` of {scenarios} not checked (--extension leaves out a Function {they})"
                ));
            }
            if !ran_nowhere.is_empty() {
                said.push_str(&format!(
                    ", {} ran in no scenario",
                    count(ran_nowhere.len(), "Function", "Functions")
                ));
            }
            println!(
                "checked {} of {} in {:.1} s: {said}",
                report.checked,
                count(report.total, "scenario", "scenarios"),
                report.elapsed.as_secs_f64(),
            );
        }
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// An expectation that is not met, as `app check` prints it: the scenario and the assertion,
/// then what the scenario expects and what the checkout has, as they are written in it.
fn print_unmet(unmet: &crate::server::Unmet) {
    let step = if unmet.step == "completion" {
        String::new()
    } else {
        format!(" (step: {})", unmet.step)
    };
    let expected = compact(&unmet.unmet.expected);
    let expected = match unmet.unmet.assertion.rsplit_once('.') {
        Some((_, "includes")) => format!("an entry matching {expected}"),
        Some((_, "excludes")) => format!("no entry matching {expected}"),
        _ => expected,
    };
    println!(
        "expect   {}: {}{step}",
        unmet.scenario, unmet.unmet.assertion
    );
    println!("         expected: {expected}");
    println!("         actual:   {}", compact(&unmet.unmet.actual));
}

/// A JSON value on one line, as one writes it by hand: `{"handle": "pickup", "cost": "0.00"}`.
fn compact(value: &serde_json::Value) -> String {
    use serde_json::Value as Json;
    match value {
        Json::Array(items) => format!(
            "[{}]",
            items.iter().map(compact).collect::<Vec<_>>().join(", ")
        ),
        Json::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(key, value)| format!("{}: {}", Json::from(key.as_str()), compact(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => other.to_string(),
    }
}
