use std::path::Path;
use std::process::ExitCode;

use lsf_app::checkout::pipeline;
use lsf_app::checkout::{BuyerJourneyStep, CheckoutResult};
use lsf_app::config::Selection;
use lsf_core::Session;
use lsf_core::diagnostics::{Diagnostics, Severity};
use lsf_core::store::load::{OVERLAY_FILE, load_with_overlay};
use lsf_core::store::model::SessionInput;
use lsf_core::store::validate::{FileKind, validate};
use serde_json::Value as Json;

use crate::app::App;
use crate::checkout::{self, display_money};
use crate::output::{count, print_diagnostics};
use crate::server::apply_session_input;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Subcommand,
}

#[derive(clap::Subcommand)]
enum Subcommand {
    /// Run the Functions of an app once, without a server: builds the input of each from the
    /// store data, runs them in checkout order, and prints each input, output and log with the
    /// checkout that results. Exits with 1 when a Function fails to run.
    Run(RunArgs),
    /// Build the Functions of an app, as `shopify app build` does. A Function written in
    /// JavaScript or TypeScript without a build command (as Shopify's CLI generates them) is
    /// built by lsf, without Node: the GraphQL typegen (`generated/api.ts`), one JavaScript file
    /// (`dist/function.js`), then its Wasm module. The others run their
    /// `[extensions.build] command`.
    Build(BuildArgs),
    /// Write the GraphQL schema of a Function's API, at the API version its
    /// `shopify.extension.toml` declares, to its `schema.graphql`, as `shopify app function
    /// schema` does: from the schemas lsf carries, without the network. Run it in the Function's
    /// folder, or name the Function (or give `--path`).
    Schema(schema::SchemaArgs),
    /// Write the GraphQL types of a JavaScript or TypeScript Function, as `shopify app function
    /// typegen` does: the `codegen` section of its `package.json` (graphql-codegen's config),
    /// written as graphql-codegen writes it (`generated/api.ts`), from its `schema.graphql` or
    /// lsf's schema of its API version, offline. `lsf app function build` does it too. Run it in
    /// the Function's folder, or name the Functions; every JavaScript Function without either.
    Typegen(BuildArgs),
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Format {
    /// Each run, then the checkout.
    Text,
    /// The checkout as the checkout page reads it (`lines`, `discounts`, `deliveryGroups`,
    /// `errors`, `cost`, ...), with every run in `runs`.
    Json,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Step {
    /// The buyer is looking at or changing the checkout (`CHECKOUT_INTERACTION`).
    Interaction,
    /// The buyer places the order (`CHECKOUT_COMPLETION`).
    Completion,
}

#[derive(clap::Args)]
struct BuildArgs {
    /// The Functions to build, by handle. Every Function of the app when none is given.
    #[arg(value_name = "HANDLE")]
    handles: Vec<String>,

    /// The directory of a Shopify app (the one with shopify.app.toml). Defaults to the app the
    /// current folder is in.
    #[arg(long, env = "LSF_APP", value_name = "DIR")]
    app: Option<std::path::PathBuf>,
}

#[derive(clap::Args)]
struct RunArgs {
    /// The Functions to run, by handle. Every Function of the app when none is given.
    #[arg(value_name = "HANDLE")]
    handles: Vec<String>,

    /// The directory of a Shopify app (the one with shopify.app.toml). Defaults to the app the
    /// current folder is in.
    #[arg(long, env = "LSF_APP", value_name = "DIR")]
    app: Option<std::path::PathBuf>,

    /// Build the extensions first, as `lsf app build` does: each Function (its
    /// `[extensions.build] command`, or lsf's own build of a JavaScript one), then each UI
    /// extension that names its sources.
    #[arg(long)]
    build: bool,

    /// The session to run for, as a file or as JSON: the same as `PUT /__lsf/session`
    /// (`customer`, `cart`, `country`, `company_location`). Defaults to the `session` of the
    /// store data.
    #[arg(long, value_name = "FILE|JSON")]
    session: Option<String>,

    /// The checkout scenario to run for: the name of a file of `checkouts/` in the store data.
    /// Its cart, buyer, language and settings are used, and the Functions it names. `--session`,
    /// `--checkout` and `--locale` are applied on top of it.
    #[arg(long, value_name = "NAME")]
    scenario: Option<String>,

    /// What the buyer entered, as a file or as JSON: `email`, `phone`, `shippingAddress`,
    /// `selectedDelivery`, `selectedPayment`, `discountCodes` and `metafields`.
    #[arg(long, value_name = "FILE|JSON")]
    checkout: Option<String>,

    /// The language of the buyer, by ISO code (`fr`, `pt-BR`): one of the languages of the
    /// store data. The Functions read it as `localization.language`. Defaults to the primary
    /// language.
    #[arg(long, value_name = "ISO")]
    locale: Option<String>,

    /// The step of the buyer journey the Functions run for: what a validation Function reads as
    /// `buyerJourney.step`. `completion` is the placing of the order, `interaction` the page
    /// and its updates.
    #[arg(long, value_enum, default_value = "completion")]
    step: Step,

    /// How to print the result.
    #[arg(long, value_enum, default_value = "text")]
    format: Format,

    /// Run one Function on this input instead of one built from the store data, as
    /// `shopify app function run --input` does: a JSON file (a fixture of a unit test, one
    /// `--save-input` wrote), or `-` for stdin. Name the Function, or run it in its folder.
    #[arg(long, short = 'i', value_name = "FILE|-", conflicts_with_all = ["session", "scenario", "checkout", "locale", "save_input"])]
    input: Option<String>,

    /// The target to run, by the Wasm export it calls (`cart_lines_discounts_generate_run`), as
    /// `shopify app function run --export`, or by its name (`cart.lines.discounts.generate.run`):
    /// for `--input` and `--save-input`, when the Function has several targets.
    #[arg(long, short = 'e', value_name = "EXPORT")]
    export: Option<String>,

    /// Write the input lsf built for a run to this file, as JSON, for `--input` or for the
    /// fixture of a unit test: the run of the Function named (and of `--export` when it has
    /// several targets).
    #[arg(long, value_name = "FILE")]
    save_input: Option<std::path::PathBuf>,
}

/// `lsf app function build`: builds the Functions, and says what it wrote.
fn build(args: &BuildArgs) -> Result<ExitCode, String> {
    checkout::run_build(
        args.app.as_deref(),
        &args.handles,
        "lsf app function build",
        checkout::build_functions,
    )
}

/// `lsf app function typegen`: the types of the JavaScript Functions named, the one of the
/// current folder, or all of them.
fn typegen(args: &BuildArgs) -> Result<ExitCode, String> {
    let app_dir = checkout::app_of(args.app.as_deref(), "lsf app function typegen")?;
    let app = lsf_app::config::load(&app_dir)?;
    let current = std::env::current_dir().map_err(|error| error.to_string())?;
    let functions: Vec<&lsf_app::config::FunctionExtension> = if !args.handles.is_empty() {
        args.handles
            .iter()
            .map(|handle| {
                schema::function_of(&app, Some(handle), &current, "lsf app function typegen")
            })
            .collect::<Result<_, _>>()?
    } else if let Ok(function) =
        schema::function_of(&app, None, &current, "lsf app function typegen")
    {
        vec![function]
    } else {
        app.functions
            .iter()
            .filter(|function| function.build() == Some(lsf_app::config::Build::JavaScript))
            .collect()
    };
    if functions.is_empty() {
        return Err("the app has no JavaScript Function to write the types of".to_string());
    }
    let mut failed = false;
    for function in functions {
        if function.build() != Some(lsf_app::config::Build::JavaScript) {
            eprintln!(
                "typegen: {}: not a JavaScript Function built by lsf: {}",
                function.handle,
                if function.dir.join("Cargo.toml").is_file() {
                    "a Rust Function's types come from the `typegen` macro, at its build"
                } else {
                    "its build command writes its types"
                }
            );
            continue;
        }
        match lsf_app::checkout::javascript::typegen(function) {
            Ok(Some(path)) => eprintln!("typegen: {}: {}", function.handle, path.display()),
            Ok(None) => eprintln!(
                "typegen: {}: its package.json has no `codegen` section: nothing to write",
                function.handle
            ),
            Err(error) => {
                eprintln!("error: the typegen of {} failed: {error}", function.handle);
                failed = true;
            }
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// JSON given on the command line, or the path of a file that holds it.
fn read_json(what: &str, value: &str) -> Result<Json, String> {
    let text = if value.trim_start().starts_with(['{', '[']) {
        value.to_string()
    } else {
        std::fs::read_to_string(value).map_err(|error| format!("--{what}: {value}: {error}"))?
    };
    serde_json::from_str(&text).map_err(|error| format!("--{what}: not valid JSON: {error}"))
}

pub fn run(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_theme::Preview],
    args: Args,
) -> Result<ExitCode, String> {
    let args = match args.command {
        Subcommand::Run(args) => args,
        Subcommand::Build(args) => return build(&args),
        Subcommand::Schema(args) => return schema::run(&args),
        Subcommand::Typegen(args) => return typegen(&args),
    };
    if let Some(input) = &args.input {
        return run_on_input(&args, input);
    }
    let Ran {
        app: shopify_app,
        result,
        scenario,
        discounted,
        named,
        forced,
    } = compute(theme, data, previews, &args)?;
    if let Some(file) = &args.save_input {
        save_input(&result, &args, file)?;
    }
    let failed = result.runs.iter().any(|run| run.error.is_some());
    // A Function asked for by name that did not run is a failure, with why.
    let not_run: Vec<(String, String)> = args
        .handles
        .iter()
        .filter_map(|handle| shopify_app.function(handle))
        .filter(|function| !result.runs.iter().any(|run| run.handle == function.handle))
        .map(|function| {
            let reason = match &named {
                Some((file, handles)) if !handles.contains(&function.handle) => format!(
                    "the scenario's `extensions` leaves it out: add \"{}\" to the `extensions` \
                     of {file}, or remove the list to run every extension",
                    function.handle
                ),
                _ => why_not_run(&shopify_app, function, &discounted, result.lines.is_empty()),
            };
            (function.handle.clone(), reason)
        })
        .collect();

    match args.format {
        Format::Json => {
            let mut json = serde_json::to_value(&result).map_err(|error| error.to_string())?;
            json["scenario"] = scenario.clone().map_or(
                Json::Null,
                |(name, title)| serde_json::json!({"name": name, "title": title}),
            );
            json["diagnostics"] = Json::Array(
                shopify_app
                    .diagnostics
                    .iter()
                    .map(|diagnostic| Json::String(diagnostic.to_string()))
                    .collect(),
            );
            json["forced"] = Json::Array(
                forced
                    .iter()
                    .map(|(handle, reason)| serde_json::json!({"handle": handle, "reason": reason}))
                    .collect(),
            );
            json["notRun"] = Json::Array(
                not_run
                    .iter()
                    .map(|(handle, reason)| serde_json::json!({"handle": handle, "reason": reason}))
                    .collect(),
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&json).map_err(|error| error.to_string())?
            );
        }
        Format::Text => {
            if let Some((name, title)) = &scenario {
                println!("== scenario {name}: {title}\n");
            }
            for (handle, reason) in &forced {
                println!(
                    "Named: {handle} runs because this run names it, but the checkout of the store data does not run it: {reason}. This command reads the store data on disk, not a running lsf, where the app may have created or enabled one; `{}` runs what the store data's checkout runs.",
                    checkout::as_typed("lsf app function run")
                );
            }
            if !forced.is_empty() {
                println!();
            }
            print_text(&result);
        }
    }
    for (handle, reason) in &not_run {
        eprintln!("not run: {handle}: {reason}");
    }
    Ok(if failed || !not_run.is_empty() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// The target of `function` that `--export` names (by its export or its name), else its only
/// one.
fn target_of<'a>(
    function: &'a lsf_app::config::FunctionExtension,
    export: Option<&str>,
) -> Result<&'a lsf_app::config::FunctionTarget, String> {
    let listed = || {
        function
            .targets
            .iter()
            .map(|target| format!("{} ({})", target.export, target.target))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match export {
        Some(export) => function
            .targets
            .iter()
            .find(|target| target.export == export || target.target == export)
            .ok_or_else(|| {
                format!(
                    "--export {export}: {} has no such target (it has {})",
                    function.handle,
                    listed()
                )
            }),
        None => match function.targets.as_slice() {
            [one] => Ok(one),
            [] => Err(format!("{} has no target", function.handle)),
            _ => Err(format!(
                "{} has several targets: say which with `--export` ({})",
                function.handle,
                listed()
            )),
        },
    }
}

/// `--input`: runs one Function on the input given, and prints its run. A folder runs each of
/// its `*.json` fixtures, as the unit tests of Shopify's templates do.
fn run_on_input(args: &RunArgs, input: &str) -> Result<ExitCode, String> {
    let app_dir = &checkout::app_of(args.app.as_deref(), "lsf app function run")?;
    if args.handles.len() > 1 {
        return Err("--input runs one Function: name one".to_string());
    }
    let open = |extensions: Vec<String>, build: bool| {
        checkout::open(
            &checkout::AppArgs {
                app: Some(app_dir.clone()),
                extensions,
                build,
            },
            app_dir,
        )
    };
    // The Function first, then `--build` builds it alone: the other extensions of the app are
    // none of this run's business.
    let current = std::env::current_dir().map_err(|error| error.to_string())?;
    let handle = {
        let (shopify_app, _) = open(args.handles.clone(), false)?;
        input_function(&shopify_app, args.handles.first(), input, &current)?
            .handle
            .clone()
    };
    let (shopify_app, selection) = open(vec![handle.clone()], args.build)?;
    // Built first when its files changed after its module was built, as Shopify's CLI does.
    let shopify_app = if args.build {
        shopify_app
    } else {
        checkout::freshen(app_dir, shopify_app, &selection)?
    };
    let function = schema::function_of(
        &shopify_app,
        Some(&handle),
        &current,
        "lsf app function run --input <file>",
    )?;
    // A wrong `--export` is a wrong option (2); a fixture that names a target the Function
    // does not have is the fixture's to fix (1).
    if let Some(export) = &args.export {
        target_of(function, Some(export))?;
    }
    if input != "-" && Path::new(input).is_dir() {
        return run_fixtures(args, function, Path::new(input));
    }
    let text = if input == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
            .map_err(|error| format!("--input -: {error}"))?;
        text
    } else {
        std::fs::read_to_string(input).map_err(|error| format!("--input {input}: {error}"))?
    };
    let fixture = Fixture::read(function, args.export.as_deref(), input, &text)
        .map_err(crate::output::fixable)?;
    let ran = fixture.run(function);
    match args.format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&ran.to_json()).map_err(|error| error.to_string())?
        ),
        Format::Text => {
            print_run(&ran.run);
            if ran.run.error.is_none() && fixture.expected.is_some() {
                println!("the output is the one the fixture expects");
            }
        }
    }
    Ok(if ran.run.error.is_some() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// The Function `--input` runs: the one named, else the one whose folder holds the input
/// (`extensions/discount/tests/fixtures/`) or the current folder, else the app's only one.
fn input_function<'a>(
    app: &'a lsf_app::config::App,
    named: Option<&String>,
    input: &str,
    current: &Path,
) -> Result<&'a lsf_app::config::FunctionExtension, String> {
    let command = "lsf app function run --input <file>";
    if let Some(handle) = named {
        return schema::function_of(app, Some(handle), current, command);
    }
    let around_input = (input != "-")
        .then(|| current.join(input))
        .and_then(|path| schema::function_of(app, None, &path, command).ok());
    if let Some(function) = around_input {
        return Ok(function);
    }
    match schema::function_of(app, None, current, command) {
        Ok(function) => Ok(function),
        Err(_) if app.functions.len() == 1 => Ok(&app.functions[0]),
        Err(error) => Err(error),
    }
}

/// An input to run a Function on: a fixture of the unit tests of Shopify's templates
/// (`{"payload": {"export", "target", "input", "output"}}`), whose target is the one it names
/// and whose output the run must answer, or an input alone.
struct Fixture<'a> {
    target: &'a lsf_app::config::FunctionTarget,
    input: Json,
    expected: Option<Json>,
}

/// A run on a [`Fixture`], with what of its output is not the one the fixture expects.
struct FixtureRun {
    run: lsf_app::checkout::FunctionRun,
    mismatches: Vec<Mismatch>,
}

impl FixtureRun {
    /// The run as `--format json` prints it, with `mismatches` when the fixture expects
    /// another output.
    fn to_json(&self) -> Json {
        let mut json = serde_json::to_value(&self.run).unwrap_or(Json::Null);
        if !self.mismatches.is_empty() {
            json["mismatches"] = Json::Array(
                self.mismatches
                    .iter()
                    .map(|mismatch| {
                        serde_json::json!({
                            "path": mismatch.path,
                            "expected": mismatch.expected,
                            "actual": mismatch.actual,
                        })
                    })
                    .collect(),
            );
        }
        json
    }
}

impl<'a> Fixture<'a> {
    /// The fixture in `text` (from `source`, as messages name it) for `function`. `export` is
    /// `--export`. A fixture whose `target` or `export` is not one of the Function's targets
    /// (or not the one `--export` names) is an error, not left aside.
    fn read(
        function: &'a lsf_app::config::FunctionExtension,
        export: Option<&str>,
        source: &str,
        text: &str,
    ) -> Result<Fixture<'a>, String> {
        let json: Json = serde_json::from_str(text)
            .map_err(|error| format!("--input {source}: not valid JSON: {error}"))?;
        let Some(payload) = json
            .get("payload")
            .filter(|payload| payload.get("input").is_some())
        else {
            return Ok(Fixture {
                target: target_of(function, export)?,
                input: json,
                expected: None,
            });
        };
        let named_target = payload["target"].as_str();
        let named_export = payload["export"].as_str();
        let listed = || {
            function
                .targets
                .iter()
                .map(|target| format!("{} ({})", target.target, target.export))
                .collect::<Vec<_>>()
                .join(", ")
        };
        // `cart_lines_discounts_generate_run` is `cart-lines-discounts-generate-run`.
        let same_export = |a: &str, b: &str| a.replace('_', "-") == b.replace('_', "-");
        let target = match (export, named_target) {
            (Some(export), _) => target_of(function, Some(export))?,
            (None, Some(name)) => function
                .targets
                .iter()
                .find(|target| target.target == name)
                .ok_or_else(|| {
                    format!(
                        "--input {source}: the fixture's target `{name}` is not a target of {} (it has {})",
                        function.handle,
                        listed()
                    )
                })?,
            (None, None) => match named_export {
                Some(name) => function
                    .targets
                    .iter()
                    .find(|target| same_export(&target.export, name))
                    .ok_or_else(|| {
                        format!(
                            "--input {source}: the fixture's export `{name}` is not an export of {} (it has {})",
                            function.handle,
                            listed()
                        )
                    })?,
                None => target_of(function, None)?,
            },
        };
        if let Some(name) = named_target
            && name != target.target
        {
            return Err(format!(
                "--input {source}: the fixture's target is `{name}`, not `{}` that --export names",
                target.target
            ));
        }
        if let Some(name) = named_export
            && !same_export(name, &target.export)
        {
            return Err(format!(
                "--input {source}: the fixture's export `{name}` is not the export of `{}`, `{}`",
                target.target, target.export
            ));
        }
        Ok(Fixture {
            target,
            input: payload["input"].clone(),
            expected: payload.get("output").cloned(),
        })
    }

    /// Runs `function` on it, and compares its output with the one it expects.
    fn run(&self, function: &lsf_app::config::FunctionExtension) -> FixtureRun {
        let mut run = pipeline::run_with_input(function, self.target, self.input.clone());
        let mut mismatches = Vec::new();
        if let (None, Some(expected), Some(output)) = (&run.error, &self.expected, &run.output) {
            json_diff("", expected, output, &mut mismatches);
            if !mismatches.is_empty() {
                run.error = Some(format!(
                    "the output is not the one the fixture expects:\n{}",
                    describe_mismatches(&mismatches)
                ));
            }
        }
        FixtureRun { run, mismatches }
    }
}

/// `--input <folder>`: runs `function` on each `*.json` fixture of `dir`, as the unit tests of
/// Shopify's templates (`tests/fixtures/`), and says which pass. Exits with 1 when one fails.
fn run_fixtures(
    args: &RunArgs,
    function: &lsf_app::config::FunctionExtension,
    dir: &Path,
) -> Result<ExitCode, String> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|error| format!("--input {}: {error}", dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "--input {}: no fixture (`*.json`) in this folder",
            dir.display()
        ));
    }
    let mut results = Vec::new();
    for file in &files {
        let name = file.display().to_string();
        let outcome = std::fs::read_to_string(file)
            .map_err(|error| format!("--input {name}: {error}"))
            .and_then(|text| Fixture::read(function, args.export.as_deref(), &name, &text))
            .map(|fixture| fixture.run(function));
        // Each line names its file already.
        let outcome = outcome.map_err(|error| {
            error
                .strip_prefix(&format!("--input {name}: "))
                .map_or(error.clone(), str::to_string)
        });
        results.push((name, outcome));
    }
    let failed = results
        .iter()
        .filter(|(_, outcome)| !matches!(outcome, Ok(ran) if ran.run.error.is_none()))
        .count();
    match args.format {
        Format::Json => {
            let fixtures: Vec<Json> = results
                .iter()
                .map(|(file, outcome)| match outcome {
                    Ok(ran) => serde_json::json!({
                        "file": file,
                        "ok": ran.run.error.is_none(),
                        "run": ran.to_json(),
                    }),
                    Err(error) => serde_json::json!({"file": file, "ok": false, "error": error}),
                })
                .collect();
            let json = serde_json::json!({
                "ok": failed == 0,
                "handle": function.handle,
                "passed": results.len() - failed,
                "failed": failed,
                "fixtures": fixtures,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&json).map_err(|error| error.to_string())?
            );
        }
        Format::Text => {
            for (file, outcome) in &results {
                match outcome {
                    Ok(ran) => match &ran.run.error {
                        None => println!("pass     {file}"),
                        Some(error) => {
                            let mut lines = error.lines();
                            println!("fails    {file}: {}", lines.next().unwrap_or_default());
                            for line in lines {
                                println!("         {line}");
                            }
                            if !ran.run.logs.trim().is_empty() {
                                println!("         logs:");
                                for line in ran.run.logs.trim_end().lines() {
                                    println!("           {line}");
                                }
                            }
                        }
                    },
                    Err(error) => println!("fails    {file}: {error}"),
                }
            }
            println!(
                "ran {} of {}: {} passed, {} failed",
                count(results.len(), "fixture", "fixtures"),
                function.handle,
                results.len() - failed,
                failed
            );
        }
    }
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// A place where the output of a run is not the one a fixture expects: its path in the output
/// (`operations[0].discountsAdd.candidates[0].value.percentage.value`), and the value each has
/// there (`None` where it has none).
#[derive(Debug, PartialEq)]
struct Mismatch {
    path: String,
    expected: Option<Json>,
    actual: Option<Json>,
}

/// Adds to `out` each place where `actual` is not `expected`, as JavaScript compares them
/// ([`same_json`]), under `path`.
fn json_diff(path: &str, expected: &Json, actual: &Json, out: &mut Vec<Mismatch>) {
    let key = |key: &str| {
        if path.is_empty() {
            key.to_string()
        } else {
            format!("{path}.{key}")
        }
    };
    match (expected, actual) {
        (Json::Object(expected), Json::Object(actual)) => {
            for (name, value) in expected {
                match actual.get(name) {
                    Some(other) => json_diff(&key(name), value, other, out),
                    None => out.push(Mismatch {
                        path: key(name),
                        expected: Some(value.clone()),
                        actual: None,
                    }),
                }
            }
            for (name, value) in actual {
                if !expected.contains_key(name) {
                    out.push(Mismatch {
                        path: key(name),
                        expected: None,
                        actual: Some(value.clone()),
                    });
                }
            }
        }
        (Json::Array(expected), Json::Array(actual)) => {
            for index in 0..expected.len().max(actual.len()) {
                let at = format!("{path}[{index}]");
                match (expected.get(index), actual.get(index)) {
                    (Some(a), Some(b)) => json_diff(&at, a, b, out),
                    (a, b) => out.push(Mismatch {
                        path: at,
                        expected: a.cloned(),
                        actual: b.cloned(),
                    }),
                }
            }
        }
        _ if same_json(expected, actual) => {}
        _ => out.push(Mismatch {
            path: if path.is_empty() {
                "(the output)".to_string()
            } else {
                path.to_string()
            },
            expected: Some(expected.clone()),
            actual: Some(actual.clone()),
        }),
    }
}

/// The mismatches as the text output prints them: each path, then what the fixture expects
/// and what the run answered, on lines of their own. The first 20.
fn describe_mismatches(mismatches: &[Mismatch]) -> String {
    const SHOWN: usize = 20;
    let value = |value: &Option<Json>| match value {
        Some(value) => serde_json::to_string(value).unwrap_or_default(),
        None => "(nothing)".to_string(),
    };
    let mut lines = Vec::new();
    for mismatch in mismatches.iter().take(SHOWN) {
        lines.push(format!("  {}", mismatch.path));
        lines.push(format!("    expected: {}", value(&mismatch.expected)));
        lines.push(format!("    actual:   {}", value(&mismatch.actual)));
    }
    if mismatches.len() > SHOWN {
        lines.push(format!("  … and {} more", mismatches.len() - SHOWN));
    }
    lines.join("\n")
}

/// Whether two JSON values are the same as JavaScript compares them: `10` and `10.0` are.
fn same_json(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(a), Json::Number(b)) => a.as_f64() == b.as_f64(),
        (Json::Array(a), Json::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_json(a, b))
        }
        (Json::Object(a), Json::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| same_json(value, other)))
        }
        _ => a == b,
    }
}

/// `--save-input`: writes the input of the one run the command names to `file`.
fn save_input(result: &CheckoutResult, args: &RunArgs, file: &Path) -> Result<(), String> {
    let runs: Vec<&lsf_app::checkout::FunctionRun> = result
        .runs
        .iter()
        .filter(|run| args.handles.is_empty() || args.handles.contains(&run.handle))
        .filter(|run| {
            args.export
                .as_deref()
                .is_none_or(|export| run.export == export || run.target == export)
        })
        .collect();
    let run = match runs.as_slice() {
        [run] => run,
        [] => {
            return Err(format!(
                "--save-input {}: no run to save the input of (the Function did not run here)",
                file.display()
            ));
        }
        many => {
            let listed: Vec<String> = many
                .iter()
                .map(|run| format!("{} · {} ({})", run.handle, run.target, run.export))
                .collect();
            return Err(format!(
                "--save-input {}: {} runs, save one: name its Function, and `--export` for its target ({})",
                file.display(),
                many.len(),
                listed.join(", ")
            ));
        }
    };
    let json = serde_json::to_string_pretty(&run.input).map_err(|error| error.to_string())?;
    // `tests/fixtures/` need not exist yet, as for `--save-input tests/fixtures/x.json` of the
    // quickstart.
    if let Some(parent) = file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("--save-input {}: {error}", file.display()))?;
    }
    std::fs::write(file, format!("{json}\n"))
        .map_err(|error| format!("--save-input {}: {error}", file.display()))?;
    eprintln!(
        "wrote {}: the input of {} · {}",
        file.display(),
        run.handle,
        run.target
    );
    Ok(())
}

/// What a run of the Functions comes to.
struct Ran {
    app: lsf_app::config::App,
    result: CheckoutResult,
    /// The name and the title of the scenario it ran for.
    scenario: Option<(String, String)>,
    /// The discount Functions a discount of the checkout runs.
    discounted: Vec<String>,
    /// The file of the scenario (`checkouts/b2b.json`) and its `extensions`, when it names
    /// them: the Functions it leaves out do not run.
    named: Option<(String, Vec<String>)>,
    /// The Functions named on the command line that the shop's checkout would not run, for want
    /// of an enabled owner, and why: they ran anyway, as named, and the output says so.
    forced: Vec<(String, String)>,
}

/// Why a Function asked for by name did not run: what the app says is wrong with it, a
/// discount Function no discount names, or targets the local checkout does not run.
fn why_not_run(
    app: &lsf_app::config::App,
    function: &lsf_app::config::FunctionExtension,
    discounted: &[String],
    empty_cart: bool,
) -> String {
    use lsf_app::config::TargetKind;
    if let Some(diagnostic) = app.diagnostics.iter().find(|diagnostic| {
        diagnostic.extension.as_deref() == Some(function.handle.as_str())
            && diagnostic.severity == lsf_app::config::Severity::Error
    }) {
        return diagnostic.message.clone();
    }
    let kinds: Vec<TargetKind> = function.targets.iter().map(|target| target.kind).collect();
    if !kinds.is_empty() && kinds.iter().all(|kind| *kind == TargetKind::Unsupported) {
        return "its targets are not run by the local checkout".to_string();
    }
    let discount = kinds.iter().any(|kind| {
        matches!(
            kind,
            TargetKind::LinesDiscounts
                | TargetKind::ProductDiscount
                | TargetKind::OrderDiscount
                | TargetKind::DeliveryDiscounts
                | TargetKind::ShippingDiscount
        )
    });
    if discount && !discounted.contains(&function.handle) {
        return format!(
            "a discount Function runs for a discount of the store, and none names it: add one to \
             `checkout.discounts` (store.json, or the scenario), e.g. {} (automatic), with \"code\" \
             for a code",
            crate::checkout::discount_example(function)
        );
    }
    if empty_cart {
        return "the cart is empty: give one with `--scenario <name>` or `--session '{\"cart\": …}'`"
            .to_string();
    }
    "it had nothing to run on in this checkout (its target does not apply to this cart)".to_string()
}

/// Builds the input of each Function from the store data and runs them, for the step, the
/// language and the checkout the arguments say.
fn compute(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_theme::Preview],
    args: &RunArgs,
) -> Result<Ran, String> {
    // The app the current folder is in when `--app` is not given.
    let app_dir = &checkout::app_of(args.app.as_deref(), "lsf app function run")?;
    let app_args = checkout::AppArgs {
        app: Some(app_dir.clone()),
        extensions: args.handles.clone(),
        build: args.build,
    };
    let (shopify_app, selection) = checkout::open(&app_args, app_dir)?;
    if let Some(handle) = args
        .handles
        .iter()
        .find(|handle| shopify_app.ui_extension(handle).is_some())
    {
        return Err(format!(
            "`{handle}` is a UI extension, not a Function: `{} app function run` runs Functions",
            lsf_core::util::command()
        ));
    }
    checkout::print_app_diagnostics(&shopify_app, &selection);

    // `--session`, as `PUT /__lsf/session` takes it: its `data` goes on top of the store data
    // for this run (and `throttle`, a matter of a server, is left aside).
    let mut session_json = match &args.session {
        Some(value) => Some(read_json("session", value)?),
        None => None,
    };
    let overlay = session_json
        .as_mut()
        .and_then(Json::as_object_mut)
        .and_then(|object| {
            object.remove("throttle");
            object.remove("data")
        });

    // The app's store data when the folder lsf runs in has none.
    let data = crate::app::data_with_app(theme, data, Some(app_dir));
    let local = App::open_without_theme(theme, data.as_deref(), previews)?;
    let (store, mut diagnostics) =
        load_with_overlay(&local.source(), &local.load_options(), overlay.as_ref());
    for item in &mut diagnostics.items {
        if item.file == OVERLAY_FILE {
            item.file = "--session data".to_string();
        }
    }
    print_diagnostics(&diagnostics);
    // A scenario with errors is reported, and refused when it is the one asked for; the others
    // do not matter to this run.
    if diagnostics
        .items
        .iter()
        .any(|item| item.severity == Severity::Error && !item.file.starts_with("checkouts/"))
    {
        return Err("the store data has errors (see above)".to_string());
    }

    let scenario = match &args.scenario {
        Some(name) => Some(scenario_to_run(&store, name)?),
        None => None,
    };
    let selection = match scenario {
        Some(scenario) => checkout::narrow_to_scenario(&shopify_app, selection, scenario)?,
        None => selection,
    };
    // A Function whose files changed after its module was built is built first, as
    // `shopify app function run` builds before it runs (`--build` built them all already).
    let shopify_app = if args.build {
        shopify_app
    } else {
        checkout::freshen(app_dir, shopify_app, &selection)?
    };

    let mut session = Session::initial(&store);
    let mut entered = None;
    if let Some(scenario) = scenario {
        let (state, problems) = checkout::apply_scenario(&store, scenario, &mut session);
        if problems.has_errors() {
            print_diagnostics(&problems);
            return Err(format!(
                "the scenario {} is not valid (see above)",
                scenario.name
            ));
        }
        entered = Some(state);
    }
    if let Some(json) = session_json {
        let mut problems = Diagnostics::new();
        validate(FileKind::Session, "--session", &json, "", &mut problems);
        if !problems.has_errors() {
            let input: SessionInput =
                serde_json::from_value(json).map_err(|error| format!("--session: {error}"))?;
            apply_session_input(&store, &input, &mut session, &mut problems, "--session");
        }
        if problems.has_errors() {
            print_diagnostics(&problems);
            return Err("--session is not valid (see above)".to_string());
        }
    }
    // What the scenario had the buyer enter, with what `--checkout` says on top.
    let mut state = entered.unwrap_or_default();
    if let Some(value) = &args.checkout {
        let mut merged = serde_json::to_value(&state).map_err(|error| error.to_string())?;
        match (read_json("checkout", value)?, merged.as_object_mut()) {
            (Json::Object(given), Some(merged)) => merged.extend(given),
            (_, _) => return Err("--checkout: must be a JSON object".to_string()),
        }
        let scenario_name = state.scenario.clone();
        state = serde_json::from_value(merged).map_err(|error| format!("--checkout: {error}"))?;
        state.scenario = scenario_name;
    }
    if let (None, Some(language)) = (&args.locale, scenario.and_then(|s| s.language.clone())) {
        state.language = Some(language);
    }
    if let Some(locale) = &args.locale {
        let language = store
            .languages
            .iter()
            .find(|language| language.iso_code.eq_ignore_ascii_case(locale))
            .ok_or_else(|| {
                let known: Vec<&str> = store
                    .languages
                    .iter()
                    .map(|language| language.iso_code.as_str())
                    .collect();
                format!(
                    "--locale {locale}: the store has no such language (it has {})",
                    known.join(", ")
                )
            })?;
        state.language = Some(language.iso_code.clone());
    }
    state.step = match args.step {
        Step::Interaction => BuyerJourneyStep::Interaction,
        Step::Completion => BuyerJourneyStep::Completion,
    };

    let mut config = checkout::config_for(&store, scenario.map(|scenario| scenario.name.as_str()));
    // The Functions named on the command line run, whether or not the shop has an owner of
    // them, as `shopify app function run` runs one: without them, the checkout's. Each one the
    // checkout of the store data would leave out is said, so that this run is not taken for the
    // checkout. The store data is the files: an owner the app created in a running lsf, through
    // the Admin API, is not in them.
    let mut forced = Vec::new();
    if !args.handles.is_empty()
        && let Some(owners) = config.owners.take()
    {
        for handle in &args.handles {
            let Some(owner) = shopify_app
                .function(handle)
                .and_then(|function| function.owner_type())
            else {
                continue;
            };
            // `PaymentCustomization` → `payment customization`.
            let owner = owner
                .chars()
                .enumerate()
                .flat_map(|(index, c)| {
                    let space = (index > 0 && c.is_ascii_uppercase()).then_some(' ');
                    space.into_iter().chain(c.to_lowercase())
                })
                .collect::<String>();
            let reason = match owners.get(handle) {
                Some(true) => continue,
                Some(false) => format!("its {owner} is disabled"),
                None => format!("it has no {owner} of it"),
            };
            forced.push((handle.clone(), reason));
        }
    }
    let functions_only =
        Selection::only(selection.functions(&shopify_app).map(|f| f.handle.clone()));
    let result = pipeline::compute(
        &shopify_app,
        &functions_only,
        &store,
        &session,
        &state,
        &config,
    );
    let named = scenario.and_then(|scenario| {
        let handles = scenario.extensions.clone()?;
        Some((scenario.file.clone(), handles))
    });
    let scenario = scenario.map(|scenario| (scenario.name.clone(), scenario.title.clone()));
    let discounted = config
        .discounts
        .iter()
        .map(|discount| discount.function.clone())
        .collect();
    Ok(Ran {
        app: shopify_app,
        result,
        scenario,
        discounted,
        named,
        forced,
    })
}

/// The scenario `--scenario` names, which must exist and be valid.
fn scenario_to_run<'a>(
    store: &'a lsf_core::Store,
    name: &str,
) -> Result<&'a lsf_core::store::CheckoutScenario, String> {
    let scenario = store.scenario(name).ok_or_else(|| {
        let known: Vec<&str> = store.scenarios.iter().map(|s| s.name.as_str()).collect();
        if known.is_empty() {
            format!(
                "--scenario {name}: the store data has no scenario (`{} app init` writes a set in `checkouts/`)",
                lsf_core::util::command()
            )
        } else {
            match lsf_core::util::closest_match(name, known.iter().copied()) {
                Some(close) => format!("--scenario {name}: no such scenario: did you mean {close}?"),
                None => format!(
                    "--scenario {name}: no such scenario (the store data has {}: {})",
                    known.len(),
                    known.join(", ")
                ),
            }
        }
    })?;
    // Its problems are among the diagnostics of the store data, printed already.
    if !scenario.problems.is_empty() {
        return Err(format!(
            "--scenario {name}: {} has errors (see above)",
            scenario.file
        ));
    }
    Ok(scenario)
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One run, as the text output prints it: its input, output, logs, instructions and error.
fn print_run(run: &lsf_app::checkout::FunctionRun) {
    println!("== {} · {} ({})", run.handle, run.target, run.export);
    println!("input:");
    println!(
        "{}",
        indent(&serde_json::to_string_pretty(&run.input).unwrap_or_default())
    );
    match &run.output {
        Some(output) => {
            println!("output:");
            println!(
                "{}",
                indent(&serde_json::to_string_pretty(output).unwrap_or_default())
            );
        }
        None => println!("output: none"),
    }
    if let Some(exchange) = &run.exchange {
        println!(
            "http: {} {} -> {}",
            exchange.request.method.as_str(),
            exchange.request.url,
            exchange.response.status
        );
        println!("response body:");
        println!("{}", indent(&exchange.response.body));
    }
    if !run.logs.trim().is_empty() {
        println!("logs:");
        println!("{}", indent(run.logs.trim_end()));
    }
    println!("instructions: {}", run.instructions);
    if let Some(error) = &run.error {
        println!("error: {error}");
    }
}

fn print_text(result: &CheckoutResult) {
    let currency = result.cost.currency_code.as_str();
    if result.runs.is_empty() {
        println!("No Function ran.");
    }
    // What the shop's owners leave out: name a Function to run it anyway.
    for idle in &result.idle {
        println!(
            "Not run: {}: {} (name it to run it anyway: `{}`)",
            idle.handle,
            idle.reason,
            checkout::as_typed(&format!("lsf app function run {}", idle.handle))
        );
    }
    if !result.idle.is_empty() {
        println!();
    }
    for run in &result.runs {
        print_run(run);
        println!();
    }

    println!("== checkout");
    for line in &result.lines {
        let title = match &line.variant_title {
            Some(variant) => format!("{} ({variant})", line.title),
            None => line.title.clone(),
        };
        let (subtotal, total) = (line.subtotal, line.total);
        if subtotal == total {
            println!(
                "  {} × {title}  {}",
                line.quantity,
                display_money(total, currency)
            );
        } else {
            println!(
                "  {} × {title}  {} → {}",
                line.quantity,
                display_money(subtotal, currency),
                display_money(total, currency)
            );
        }
    }
    for discount in &result.discounts {
        println!(
            "  discount {}{}: -{} ({:?}, {})",
            discount.title,
            discount
                .code
                .as_deref()
                .map(|code| format!(" [{code}]"))
                .unwrap_or_default(),
            display_money(discount.amount, currency),
            discount.class,
            discount.function
        );
    }
    for skipped in &result.skipped_discounts {
        println!(
            "  discount not applied {}{}: {} ({})",
            skipped.title,
            skipped
                .code
                .as_deref()
                .map(|code| format!(" [{code}]"))
                .unwrap_or_default(),
            skipped.reason,
            skipped.function
        );
    }
    for group in &result.delivery_groups {
        for option in &group.options {
            let mark = if group.selected.as_deref() == Some(option.handle.as_str()) {
                "*"
            } else {
                " "
            };
            println!(
                "  delivery {mark} {} ({}): {}",
                option.title,
                option.handle,
                display_money(option.cost, currency)
            );
        }
    }
    // Each method with where it is still offered once the Functions ran: the list of the
    // checkout (`PAYMENT_METHOD`), the express buttons (`ACCELERATED_CHECKOUT`), or both.
    let mut methods: Vec<&lsf_app::checkout::PaymentMethod> =
        result.payment_methods.iter().collect();
    for method in &result.express_methods {
        if !methods.iter().any(|known| known.handle == method.handle) {
            methods.push(method);
        }
    }
    if !methods.is_empty() {
        let described: Vec<String> = methods
            .iter()
            .map(|method| {
                let placements: Vec<&str> = method
                    .placements
                    .iter()
                    .map(|placement| placement.graphql())
                    .collect();
                format!("{} [{}]", method.name, placements.join(", "))
            })
            .collect();
        println!("  payment: {}", described.join(", "));
    }
    for error in &result.errors {
        println!(
            "  error{}: {}",
            error
                .function
                .as_deref()
                .map(|handle| format!(" [{handle}]"))
                .unwrap_or_default(),
            error.message
        );
    }
    let cost = &result.cost;
    println!(
        "  subtotal {} · discounts {}{} · shipping {} · tax {} · total {}",
        display_money(cost.subtotal, currency),
        // No discount is 0.00, not -0.00.
        if cost.discounts == 0 { "" } else { "-" },
        display_money(cost.discounts, currency),
        display_money(cost.shipping, currency),
        display_money(cost.tax, currency),
        display_money(cost.total, currency)
    );
    // Only a cart with a selling plan that defers a part of the price has a balance.
    if cost.due_later > 0 {
        println!(
            "  due now {} · due later {}",
            display_money(cost.due_now, currency),
            display_money(cost.due_later, currency)
        );
    }
}

mod schema;

#[cfg(test)]
mod playground;

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn a_fixture_mismatch_names_each_path() {
        let expected =
            serde_json::json!({"operations": [{"add": {"value": 10, "message": "a"}}], "x": 1});
        let actual = serde_json::json!({"operations": [{"add": {"value": 15.0, "message": "a"}}, {"more": true}], "y": 2});
        let mut found = Vec::new();
        json_diff("", &expected, &actual, &mut found);
        let paths: Vec<&str> = found.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(
            paths,
            ["operations[0].add.value", "operations[1]", "x", "y"]
        );
        assert_eq!(found[0].expected, Some(serde_json::json!(10)));
        assert_eq!(found[1].expected, None);
        // 10 and 10.0 are the same number, as in JavaScript.
        let mut none = Vec::new();
        json_diff(
            "",
            &serde_json::json!({"v": 10}),
            &serde_json::json!({"v": 10.0}),
            &mut none,
        );
        assert!(none.is_empty(), "{none:?}");
        let text = describe_mismatches(&found);
        assert!(
            text.starts_with("  operations[0].add.value\n    expected: 10\n    actual:   15.0\n"),
            "{text}"
        );
        assert!(
            text.contains("  x\n    expected: 1\n    actual:   (nothing)"),
            "{text}"
        );
    }

    #[test]
    fn json_comes_from_the_argument_or_from_a_file() {
        assert_eq!(
            read_json("session", r#" {"country": "FR"} "#).unwrap()["country"],
            "FR"
        );
        let file =
            std::env::temp_dir().join(format!("lsf-function-run-{}.json", std::process::id()));
        std::fs::write(&file, r#"{"email": "a@b.co"}"#).unwrap();
        let read = read_json("checkout", file.to_str().unwrap());
        std::fs::remove_file(&file).unwrap();
        assert_eq!(read.unwrap()["email"], "a@b.co");
        assert!(
            read_json("checkout", "/nope/nothing.json")
                .unwrap_err()
                .starts_with("--checkout: ")
        );
        assert!(
            read_json("checkout", "{nope")
                .unwrap_err()
                .contains("not valid JSON")
        );
    }

    #[test]
    fn the_locale_is_one_of_the_languages_of_the_store() {
        let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/tests/fixtures/app");
        let root = std::env::temp_dir().join(format!("lsf-function-locale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let store = serde_json::json!({
            "products": [{"title": "Thing", "price": 3200}],
            "session": {"cart": {"items": [{"variant": "thing", "quantity": 1}]}},
        });
        std::fs::write(data.join("store.json"), store.to_string()).unwrap();
        let run_in = |locale: &str| {
            let args = Args {
                command: Subcommand::Run(RunArgs {
                    handles: vec!["quantity-limit".to_string()],
                    app: Some(app.clone()),
                    build: false,
                    session: None,
                    scenario: None,
                    checkout: None,
                    locale: Some(locale.to_string()),
                    step: Step::Completion,
                    format: Format::Json,
                    input: None,
                    export: None,
                    save_input: None,
                }),
            };
            run(&root, Some(&data), &[], args).map(|_| ())
        };
        assert_eq!(run_in("EN"), Ok(()), "the primary language, in any case");
        let error = run_in("xx").unwrap_err();
        assert!(
            error.contains("--locale xx: the store has no such language"),
            "{error}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_step_is_what_a_validation_function_reads() {
        let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/tests/fixtures/app");
        let root = std::env::temp_dir().join(format!("lsf-function-step-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join("data");
        std::fs::create_dir_all(&data).unwrap();
        // 4 in the cart, 3 allowed, but only when the order is placed.
        let store = serde_json::json!({
            "products": [{"title": "Thing", "price": 3200}],
            "session": {"cart": {"items": [{"variant": "thing", "quantity": 4}]}},
            "checkout": {"functions": {"quantity-limit": {"metafields": {"$app:config.settings": {
                "type": "json", "value": {"maxQuantity": 3, "onlyAtCompletion": true}
            }}}}},
        });
        std::fs::write(data.join("store.json"), store.to_string()).unwrap();
        let errors_at = |step: Step| {
            let args = RunArgs {
                handles: vec!["quantity-limit".to_string()],
                app: Some(app.clone()),
                build: false,
                session: None,
                scenario: None,
                checkout: None,
                locale: None,
                step,
                format: Format::Json,
                input: None,
                export: None,
                save_input: None,
            };
            let result = compute(&root, Some(&data), &[], &args).unwrap().result;
            let seen = result.runs[0].input["buyerJourney"]["step"].clone();
            (result.errors.len(), seen)
        };
        assert_eq!(
            errors_at(Step::Interaction),
            (0, serde_json::json!("CHECKOUT_INTERACTION"))
        );
        assert_eq!(
            errors_at(Step::Completion),
            (1, serde_json::json!("CHECKOUT_COMPLETION"))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn function_build_finds_the_app_the_current_folder_is_in() {
        let root = std::env::temp_dir().join(format!("lsf-app-around-{}", std::process::id()));
        let nested = root.join("extensions/bulk/src");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("shopify.app.toml"), "name = \"App\"\n").unwrap();
        assert_eq!(checkout::app_around(&nested), Some(root.clone()));
        assert_eq!(checkout::app_around(&root), Some(root.clone()));
        assert_eq!(checkout::app_around(&std::env::temp_dir()), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_step_defaults_to_the_completion() {
        #[derive(clap::Parser)]
        struct Cli {
            #[command(flatten)]
            args: Args,
        }
        let step = |extra: &[&str]| {
            let mut words = vec!["lsf", "run", "quantity-limit"];
            words.extend_from_slice(extra);
            let Subcommand::Run(run) = Cli::parse_from(words).args.command else {
                panic!("not `run`");
            };
            run.step
        };
        assert!(matches!(step(&[]), Step::Completion));
        assert!(matches!(
            step(&["--step", "interaction"]),
            Step::Interaction
        ));
        assert!(matches!(step(&["--step", "completion"]), Step::Completion));
    }

    /// Runs `lsf app function run` on the fixture app of lsf-app, for a store with one product
    /// (4 at 32.00 in the cart) and the given discount, and says whether it exits with 1.
    fn run_fails(discount: Json, handle: &str, function_settings: Option<Json>) -> bool {
        let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/tests/fixtures/app");
        run_fails_in(app, discount, handle, function_settings)
    }

    /// [`run_fails`] for the app in `app`.
    fn run_fails_in(
        app: std::path::PathBuf,
        discount: Json,
        handle: &str,
        function_settings: Option<Json>,
    ) -> bool {
        let root = std::env::temp_dir().join(format!(
            "lsf-function-run-{}-{}",
            std::process::id(),
            handle
        ));
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let mut checkout = serde_json::json!({ "discounts": [discount] });
        if let Some(settings) = function_settings {
            checkout["functions"] = serde_json::json!({
                handle: {"metafields": {"$app:config.settings": {"type": "json", "value": settings}}}
            });
        }
        let store = serde_json::json!({
            "products": [{"title": "Thing", "price": 3200}],
            "session": {"cart": {"items": [{"variant": "thing", "quantity": 4}]}},
            "checkout": checkout,
        });
        std::fs::write(data.join("store.json"), store.to_string()).unwrap();
        let args = Args {
            command: Subcommand::Run(RunArgs {
                handles: vec![handle.to_string()],
                app: Some(app),
                build: false,
                session: None,
                scenario: None,
                checkout: None,
                locale: None,
                step: Step::Completion,
                format: Format::Json,
                input: None,
                export: None,
                save_input: None,
            }),
        };
        let code = run(&root, Some(&data), &[], args).unwrap();
        std::fs::remove_dir_all(&root).ok();
        format!("{code:?}") == format!("{:?}", ExitCode::FAILURE)
    }

    fn volume_discount(settings: Json) -> Json {
        serde_json::json!({
            "function": "volume-discount",
            "classes": ["PRODUCT"],
            "metafields": {"$app:config.settings": {"type": "json", "value": settings}}
        })
    }

    /// One test: the runs share the scratch theme `App::open_without_theme` makes per process.
    #[test]
    fn only_a_failed_run_exits_with_1() {
        // The Function ran and gave an output, but it gives more than 100 percent off and the
        // checkout cannot apply it: the run fails.
        assert!(run_fails(
            volume_discount(serde_json::json!({"minimumQuantity": 1, "percentage": 150})),
            "volume-discount",
            None
        ));
        // The same Function with a fair discount.
        assert!(!run_fails(
            volume_discount(serde_json::json!({"minimumQuantity": 1, "percentage": 50})),
            "volume-discount",
            None
        ));
        // The cart has 4 items and the validation allows 1: the checkout cannot be placed, but
        // the Function did its job.
        assert!(!run_fails(
            serde_json::json!({"function": "volume-discount"}),
            "quantity-limit",
            Some(serde_json::json!({"maxQuantity": 1}))
        ));
        // Shopify stops a Function past 11 million instructions: the run fails, and what the
        // Function answered does not count. The same module with 1 000 turns of 8 instructions
        // is fine, with 2 million turns it does 16 million.
        let unused = serde_json::json!({"function": "spin"});
        let quick = spinning_app(1_000);
        assert!(!run_fails_in(quick.clone(), unused.clone(), "spin", None));
        let slow = spinning_app(2_000_000);
        assert!(run_fails_in(slow.clone(), unused, "spin", None));
        std::fs::remove_dir_all(quick).ok();
        std::fs::remove_dir_all(slow).ok();
    }

    /// An app with one validation Function whose module spins `turns` times (8 Wasm
    /// instructions each) and answers `{"errors": []}`, a valid output that blocks nothing.
    fn spinning_app(turns: u32) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "lsf-function-run-spin-{}-{turns}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let extension = root.join("extensions/spin");
        std::fs::create_dir_all(extension.join("src")).unwrap();
        std::fs::create_dir_all(extension.join("dist")).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/tests/fixtures/app");
        std::fs::copy(
            fixture.join("shopify.app.toml"),
            root.join("shopify.app.toml"),
        )
        .unwrap();
        let limit = fixture.join("extensions/quantity-limit");
        std::fs::copy(
            limit.join("src/run.graphql"),
            extension.join("src/run.graphql"),
        )
        .unwrap();
        let toml = std::fs::read_to_string(limit.join("shopify.extension.toml")).unwrap();
        // Same Function as the fixture, with another handle and no build command.
        let toml = toml
            .replace("quantity-limit", "spin")
            .lines()
            .filter(|line| !line.trim_start().starts_with("command = "))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(extension.join("shopify.extension.toml"), toml).unwrap();
        let module = r#"
(module
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 100) "{\"errors\":[]}")
  (func (export "run") (local $i i32)
    (loop $spin
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $spin (i32.lt_u (local.get $i) (i32.const TURNS))))
    (i32.store (i32.const 0) (i32.const 100))
    (i32.store (i32.const 4) (i32.const 13))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 12)))))
"#
        .replace("TURNS", &turns.to_string());
        std::fs::write(extension.join("dist/function.wasm"), module).unwrap();
        root
    }

    /// A data directory of the demo store with the scenarios `(file, content)` in `checkouts/`.
    fn scenario_root(label: &str, scenarios: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "lsf-function-scenario-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        checkout::write_demo_scenarios(&root.join("data"), scenarios);
        root
    }

    fn run_args(handles: &[&str], scenario: Option<&str>) -> RunArgs {
        RunArgs {
            handles: handles.iter().map(|handle| handle.to_string()).collect(),
            app: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/tests/fixtures/app")),
            build: false,
            session: None,
            scenario: scenario.map(str::to_string),
            checkout: None,
            locale: None,
            step: Step::Completion,
            format: Format::Json,
            input: None,
            export: None,
            save_input: None,
        }
    }

    #[test]
    fn a_scenario_sets_up_the_run() {
        let strict = r#"{
            "title": "Too many tees",
            "session": {"country": "FR", "cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 4}]}},
            "entered": {"discount_codes": ["SAVE"], "shipping_address": {"country_code": "FR"}},
            "language": "fr",
            "checkout": {
                "shipping_rates": [{"handle": "pallet", "price": "45.00"}],
                "discounts": [{"function": "volume-discount", "code": "SAVE", "classes": ["PRODUCT"],
                    "metafields": {"$app:config.settings": {"type": "json", "value": {"minimumQuantity": 1, "percentage": 50}}}}],
                "functions": {"quantity-limit": {"enabled": true, "metafields": {"$app:config.settings": {"type": "json", "value": {"maxQuantity": 3}}}}}
            }
        }"#;
        let narrow = r#"{
            "session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 4}]}},
            "extensions": ["quantity-limit"]
        }"#;
        let root = scenario_root("run", &[("strict.json", strict), ("narrow.json", narrow)]);
        let data = root.join("data");

        let Ran {
            result, scenario, ..
        } = compute(&root, Some(&data), &[], &run_args(&[], Some("strict"))).unwrap();
        assert_eq!(
            scenario,
            Some(("strict".to_string(), "Too many tees".to_string()))
        );
        // The cart, the settings, the code and the language are the scenario's.
        assert_eq!(result.lines[0].quantity, 4);
        assert_eq!(result.delivery_groups[0].options[0].handle, "pallet");
        assert_eq!(result.discounts[0].code.as_deref(), Some("SAVE"));
        assert_eq!(result.errors.len(), 1, "{:?}", result.errors);

        // The Functions the scenario names are the ones that run.
        let result = compute(&root, Some(&data), &[], &run_args(&[], Some("narrow")))
            .unwrap()
            .result;
        let ran: Vec<&str> = result.runs.iter().map(|run| run.handle.as_str()).collect();
        assert_eq!(ran, ["quantity-limit"]);

        // `--session` and `--checkout` go on top, `--locale` wins.
        let mut args = run_args(&["quantity-limit"], Some("strict"));
        args.session =
            Some(r#"{"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}"#.into());
        args.checkout = Some(r#"{"email": "me@example.com"}"#.into());
        args.locale = Some("xx".into());
        let wrong = compute(&root, Some(&data), &[], &args)
            .map(|_| ())
            .unwrap_err();
        assert!(
            wrong.contains("--locale xx: the store has no such language"),
            "{wrong}"
        );
        args.locale = None;
        let result = compute(&root, Some(&data), &[], &args).unwrap().result;
        assert_eq!(result.lines[0].quantity, 2);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let seen = &result.runs[0].input;
        assert_eq!(seen["cart"]["buyerIdentity"]["email"], "me@example.com");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_scenario_that_cannot_run_is_explained() {
        let root = scenario_root(
            "refused",
            &[("typo.json", r#"{"titel": "x"}"#), ("ok.json", "{}")],
        );
        let data = root.join("data");
        let error = |name: &str| {
            compute(&root, Some(&data), &[], &run_args(&[], Some(name)))
                .map(|_| ())
                .unwrap_err()
        };
        let missing = error("nope");
        assert!(
            missing.contains("--scenario nope: no such scenario (the store data has 2: ok, typo)"),
            "{missing}"
        );
        let broken = error("typo");
        assert!(
            broken.contains("--scenario typo: checkouts/typo.json has errors"),
            "{broken}"
        );
        std::fs::remove_dir_all(&root).ok();

        // Without any scenario the message says where they come from.
        let root = scenario_root("none", &[]);
        let data = root.join("data");
        let none = compute(&root, Some(&data), &[], &run_args(&[], Some("x")))
            .map(|_| ())
            .unwrap_err();
        assert!(none.contains("has no scenario"), "{none}");
        std::fs::remove_dir_all(&root).ok();
    }
}
