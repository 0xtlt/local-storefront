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
    /// its Functions run and its checkout UI extensions show, its customer account
    /// (`/account`), where its customer account UI extensions show, and the Admin API of its
    /// backend, with OAuth, webhooks and lsf's admin. `serve --app` serves them with a theme.
    /// It takes the options of `serve`.
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
    /// `{ok, state, checked, total, elapsedMs, failures: [{handle, target, error, logs, scenarios}], invalid: [{scenario, file, problems}], expectations: {checked, unchecked: [scenario], unmet: [{scenario, file, step, assertion, expected, actual}]}, ranInNoScenario: [handle], notOffered: [{handle, reason}], stale: [{handle, file}], themeExtensions: [{handle, severity, message}]}`, and `error` when there is no scenario. `state` is `done`, or `partial` when the time ran out. A Function target that fails in several ways has a failure for each error, with the scenarios of that error; `logs` is what the Function logged in the first of them. `expectations.checked` counts the expectations of the scenarios' `expect`, met or not; `unchecked` names the scenarios whose `expect` was not checked, as `--extension` leaves out a Function they run; `assertion` is the list and the rule (`express_methods.excludes`) or the amount (`totals.total`), `expected` what the scenario says, `actual` what the checkout has. `ranInNoScenario` is filled only when every scenario ran. `notOffered` has the extensions the checkout does not offer (an `api_version` lsf does not run, no module, no target it has), `stale` the Functions whose `file` changed after their module was built, `themeExtensions` what Shopify rejects (`error`) or advises against in the theme app extensions.
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

/// The extensions the store data names: in the `extensions` of a scenario, in a discount of
/// `checkout.discounts` (of the store data or of a scenario), or with `--extension`.
fn named_extensions(store: &lsf_core::Store, extensions: &[String]) -> Vec<String> {
    let mut named: Vec<String> = extensions.to_vec();
    for scenario in &store.scenarios {
        named.extend(scenario.extensions.iter().flatten().cloned());
        named.extend(
            scenario
                .checkout
                .discounts
                .iter()
                .map(|discount| discount.function.clone()),
        );
    }
    named.extend(
        store
            .checkout
            .discounts
            .iter()
            .map(|discount| discount.function.clone()),
    );
    named
}

/// Why the checkout does not offer the extension `handle`, which `app check` covers: an error
/// of the app (an `api_version` lsf does not run, an input query it cannot read), no module or
/// bundle, or, for one the store data or `--extension` names, targets the checkout does not
/// have. A UI extension with no checkout or customer account target (an admin one) is none of
/// the checkout's business, unless named.
fn not_offered(app: &lsf_app::config::App, handle: &str, named: bool) -> Option<String> {
    use lsf_app::config::{Severity, TargetKind};
    let diagnostics = || {
        app.diagnostics
            .iter()
            .filter(move |diagnostic| diagnostic.extension.as_deref() == Some(handle))
    };
    if let Some(function) = app.function(handle) {
        if let Some(error) = diagnostics().find(|d| d.severity == Severity::Error) {
            return Some(error.message.clone());
        }
        if !function.built() {
            return Some(function.missing_build());
        }
        if named
            && function
                .targets
                .iter()
                .all(|target| target.kind == TargetKind::Unsupported)
        {
            return Some(if function.targets.is_empty() {
                "the Function has no target".to_string()
            } else {
                "its targets are not run by the local checkout".to_string()
            });
        }
        return None;
    }
    let extension = app.ui_extension(handle)?;
    let placed = extension
        .targets
        .iter()
        .any(|target| lsf_app::checkout::placement::placement(&target.target).is_some());
    if !placed && !named {
        return None;
    }
    if let Some(error) = diagnostics().find(|d| d.severity == Severity::Error) {
        return Some(error.message.clone());
    }
    if !extension.built()
        && let Some(warning) = diagnostics().find(|d| d.message.starts_with("not built"))
    {
        return Some(warning.message.clone());
    }
    if !placed {
        return Some(
            diagnostics()
                .find(|d| d.message.contains("not a target") || d.message.contains("no target"))
                .map_or_else(
                    || "it has no target of the checkout or the customer account".to_string(),
                    |warning| warning.message.clone(),
                ),
        );
    }
    None
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
    // The theme app extensions named are checked as `lsf app build` checks them; the others are
    // the Functions and UI extensions the scenarios run.
    let (themes, extensions): (Vec<String>, Vec<String>) = {
        let app = lsf_app::config::load(&app_dir)?;
        args.extensions
            .iter()
            .cloned()
            .partition(|handle| app.theme_extension(handle).is_some())
    };
    // `--extension` names theme app extensions only: they are checked, and nothing else is
    // (an empty selection of the others would mean all of them).
    if !args.extensions.is_empty() && extensions.is_empty() {
        let app = lsf_app::config::load(&app_dir)?;
        return Ok(theme_extensions_only(
            &crate::theme_extensions::diagnostics(&app, &themes),
            args.format,
        ));
    }
    let (shopify_app, selection) = crate::checkout::open(
        &crate::checkout::AppArgs {
            app: Some(app_dir.clone()),
            extensions: extensions.clone(),
            build: args.build,
        },
        &app_dir,
    )?;
    // What Shopify rejects in the theme app extensions: all of them, unless `--extension`
    // names other extensions only.
    let theme_problems = if args.extensions.is_empty() || !themes.is_empty() {
        crate::theme_extensions::diagnostics(&shopify_app, &themes)
    } else {
        Vec::new()
    };
    let theme_errors = theme_problems
        .iter()
        .filter(|problem| problem.severity == lsf_app::config::Severity::Error)
        .count();
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
                extensions: extensions.clone(),
                variant: crate::checkout::Variant::Standard,
                standalone: true,
                no_functions: false,
            }),
        },
    );
    let store = state.loaded().store.clone();
    if matches!(args.format, Format::Text) {
        // The problems of a scenario that does not validate are printed with it, below.
        let mut rest = diagnostics.clone();
        rest.items.retain(|item| {
            !store
                .scenarios
                .iter()
                .any(|scenario| !scenario.problems.is_empty() && scenario.file == item.file)
        });
        crate::output::print_diagnostics(&rest);
    }
    let report = crate::server::check_scenarios(
        &state,
        &shopify_app,
        &store,
        std::time::Duration::from_secs(args.budget),
    );
    // A scenario that does not validate, or that names an extension the app does not have: it
    // does not run, so it fails the check rather than leaving it with nothing to check.
    let invalid: Vec<(&lsf_core::store::CheckoutScenario, Vec<String>)> = store
        .scenarios
        .iter()
        .filter_map(|scenario| {
            let mut problems = scenario.problems.clone();
            if problems.is_empty()
                && let Err(problem) =
                    crate::checkout::narrow_to_scenario(&shopify_app, selection.clone(), scenario)
            {
                problems.push(problem);
            }
            (!problems.is_empty()).then_some((scenario, problems))
        })
        .collect();
    let complete = report.checked == report.total;
    // No scenario, nothing checked: a check that cannot fail is not one.
    let empty = store.scenarios.is_empty();
    // The extensions the check covers that the checkout does not offer, and why: nothing
    // checks them, so the check fails rather than passing without them.
    let named = named_extensions(&store, &extensions);
    let not_offered: Vec<(String, String)> = shopify_app
        .handles()
        .into_iter()
        .filter(|handle| selection.contains(handle))
        .filter_map(|handle| {
            not_offered(
                &shopify_app,
                handle,
                named.iter().any(|name| name == handle),
            )
            .map(|reason| (handle.to_string(), reason))
        })
        .collect();
    // The Functions whose module is older than their files: what runs is not what they say.
    let stale: Vec<(String, String)> = crate::checkout::stale_functions(&shopify_app, &selection)
        .into_iter()
        .filter(|(handle, _)| !not_offered.iter().any(|(other, _)| other == handle))
        .collect();
    let ok = report.failures.is_empty()
        && invalid.is_empty()
        && report.unmet.is_empty()
        && not_offered.is_empty()
        && stale.is_empty()
        && theme_errors == 0
        && complete
        && !empty;
    // The Functions no scenario ran: nothing checks them. A warning, not a failure: a Function
    // may be meant for a case the scenarios leave aside. Only known once every scenario ran.
    let ran_nowhere: Vec<&lsf_app::config::FunctionExtension> = if complete && report.total > 0 {
        selection
            .functions(&shopify_app)
            .filter(|function| !report.ran.contains(&function.handle))
            .filter(|function| {
                !not_offered
                    .iter()
                    .any(|(handle, _)| *handle == function.handle)
            })
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
                .map(|(scenario, problems)| serde_json::json!({"scenario": scenario.name, "file": scenario.file, "problems": problems}))
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
            json["notOffered"] =
                serde_json::json!(not_offered
                .iter()
                .map(|(handle, reason)| serde_json::json!({"handle": handle, "reason": reason}))
                .collect::<Vec<_>>());
            json["stale"] = serde_json::json!(
                stale
                    .iter()
                    .map(|(handle, file)| serde_json::json!({"handle": handle, "file": file}))
                    .collect::<Vec<_>>()
            );
            json["themeExtensions"] = serde_json::json!(theme_problems
                .iter()
                .map(|problem| serde_json::json!({
                    "handle": problem.extension,
                    "severity": if problem.severity == lsf_app::config::Severity::Error { "error" } else { "warning" },
                    "message": problem.message,
                }))
                .collect::<Vec<_>>());
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
            for (scenario, problems) in &invalid {
                println!("invalid  {} ({})", scenario.name, scenario.file);
                for problem in problems {
                    for line in problem.lines() {
                        println!("         {line}");
                    }
                }
            }
            for (handle, reason) in &not_offered {
                println!("fails    {handle}: {reason}");
            }
            for (handle, file) in &stale {
                let build = match shopify_app.function(handle).and_then(|f| f.build()) {
                    Some(_) => format!(
                        "run with --build (or `{}`)",
                        crate::checkout::as_typed(&format!("lsf app build {handle}"))
                    ),
                    None => "build it again: it has no build command lsf can run".to_string(),
                };
                println!(
                    "stale    {handle}: {file} changed after its module was built, which runs the old code: {build}"
                );
            }
            for problem in &theme_problems {
                let level = if problem.severity == lsf_app::config::Severity::Error {
                    "invalid "
                } else {
                    "warning "
                };
                println!("{level} {problem}");
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
                if !not_offered.is_empty() {
                    said.push_str(&format!(
                        ", {} not offered",
                        count(not_offered.len(), "extension", "extensions")
                    ));
                }
                if !stale.is_empty() {
                    said.push_str(&format!(
                        ", {}",
                        count(stale.len(), "stale module", "stale modules")
                    ));
                }
                if theme_errors > 0 {
                    said.push_str(&format!(
                        ", {} in theme app extensions",
                        count(theme_errors, "error", "errors")
                    ));
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

/// `app check --extension <theme app extension>`: what Shopify rejects in the extensions
/// named, and no scenario.
fn theme_extensions_only(problems: &[lsf_app::config::Diagnostic], format: Format) -> ExitCode {
    let errors = problems
        .iter()
        .filter(|problem| problem.severity == lsf_app::config::Severity::Error)
        .count();
    match format {
        Format::Json => {
            let json = serde_json::json!({
                "ok": errors == 0,
                "checked": 0,
                "total": 0,
                "themeExtensions": problems
                    .iter()
                    .map(|problem| serde_json::json!({
                        "handle": problem.extension,
                        "severity": if problem.severity == lsf_app::config::Severity::Error { "error" } else { "warning" },
                        "message": problem.message,
                    }))
                    .collect::<Vec<_>>(),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&json).unwrap_or_default()
            );
        }
        Format::Text => {
            for problem in problems {
                let level = if problem.severity == lsf_app::config::Severity::Error {
                    "invalid "
                } else {
                    "warning "
                };
                println!("{level} {problem}");
            }
            println!(
                "checked the theme app extensions named: {}",
                count(errors, "error", "errors")
            );
        }
    }
    if errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
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
