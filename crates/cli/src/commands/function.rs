use std::path::Path;
use std::process::ExitCode;

use lsf_checkout::app::Selection;
use lsf_checkout::pipeline;
use lsf_checkout::{BuyerJourneyStep, CheckoutResult};
use lsf_core::Session;
use lsf_core::diagnostics::{Diagnostics, Severity};
use lsf_core::store::model::SessionInput;
use lsf_core::store::validate::{FileKind, validate};
use serde_json::Value as Json;

use crate::app::App;
use crate::checkout::{self, display_money};
use crate::output::print_diagnostics;
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

    /// The directory of a Shopify app (the one with shopify.app.toml).
    #[arg(long, env = "LSF_APP", value_name = "DIR")]
    app: Option<std::path::PathBuf>,

    /// Run the build command of each Function (`[extensions.build] command`) first.
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
}

/// `lsf function build`: builds the Functions, and says what it wrote.
fn build(args: &BuildArgs) -> Result<ExitCode, String> {
    checkout::run_build(
        args.app.as_deref(),
        &args.handles,
        "lsf function build",
        checkout::build_functions,
    )
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
    previews: &[lsf_core::Preview],
    args: Args,
) -> Result<ExitCode, String> {
    let args = match args.command {
        Subcommand::Run(args) => args,
        Subcommand::Build(args) => return build(&args),
    };
    let Ran {
        app: shopify_app,
        result,
        scenario,
        discounted,
    } = compute(theme, data, previews, &args)?;
    let failed = result.runs.iter().any(|run| run.error.is_some());
    // A Function asked for by name that did not run is a failure, with why.
    let not_run: Vec<(String, String)> = args
        .handles
        .iter()
        .filter_map(|handle| shopify_app.function(handle))
        .filter(|function| !result.runs.iter().any(|run| run.handle == function.handle))
        .map(|function| {
            (
                function.handle.clone(),
                why_not_run(&shopify_app, function, &discounted, result.lines.is_empty()),
            )
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

/// What a run of the Functions comes to.
struct Ran {
    app: lsf_checkout::app::App,
    result: CheckoutResult,
    /// The name and the title of the scenario it ran for.
    scenario: Option<(String, String)>,
    /// The discount Functions a discount of the checkout runs.
    discounted: Vec<String>,
}

/// Why a Function asked for by name did not run: what the app says is wrong with it, a
/// discount Function no discount names, or targets the local checkout does not run.
fn why_not_run(
    app: &lsf_checkout::app::App,
    function: &lsf_checkout::app::FunctionExtension,
    discounted: &[String],
    empty_cart: bool,
) -> String {
    use lsf_checkout::app::TargetKind;
    if let Some(diagnostic) = app.diagnostics.iter().find(|diagnostic| {
        diagnostic.extension.as_deref() == Some(function.handle.as_str())
            && diagnostic.severity == lsf_checkout::app::Severity::Error
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
             `checkout.discounts` (store.json, or the scenario), e.g. \
             {{\"function\": \"{}\", \"title\": \"10% off\"}} (automatic), with \"code\" for a code",
            function.handle
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
    previews: &[lsf_core::Preview],
    args: &RunArgs,
) -> Result<Ran, String> {
    // The app the current folder is in when `--app` is not given.
    let app_dir = &checkout::app_of(args.app.as_deref(), "lsf function run")?;
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
            "`{handle}` is a checkout UI extension, not a Function: `lsf function run` runs Functions"
        ));
    }
    checkout::print_app_diagnostics(&shopify_app, &selection);

    // The app's store data when the folder lsf runs in has none.
    let data = crate::app::data_with_app(theme, data, Some(app_dir));
    let local = App::open_without_theme(theme, data.as_deref(), previews)?;
    let (store, diagnostics) = local.load_store();
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
    if let Some(value) = &args.session {
        let mut problems = Diagnostics::new();
        let json = read_json("session", value)?;
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

    let config = checkout::config_for(&store, scenario.map(|scenario| scenario.name.as_str()));
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
                "--scenario {name}: the store data has no scenario (`lsf checkout init` writes a set in `checkouts/`)"
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
    if !scenario.problems.is_empty() {
        for problem in &scenario.problems {
            eprintln!("{problem}");
        }
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

fn print_text(result: &CheckoutResult) {
    let currency = result.cost.currency_code.as_str();
    if result.runs.is_empty() {
        println!("No Function ran.");
    }
    for run in &result.runs {
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
    if !result.payment_methods.is_empty() {
        let names: Vec<&str> = result
            .payment_methods
            .iter()
            .map(|method| method.name.as_str())
            .collect();
        println!("  payment: {}", names.join(", "));
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
        "  subtotal {} · discounts -{} · shipping {} · tax {} · total {}",
        display_money(cost.subtotal, currency),
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

#[cfg(test)]
mod playground;

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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
        let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../checkout/tests/fixtures/app");
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
        let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../checkout/tests/fixtures/app");
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

    /// Runs `lsf function run` on the fixture app of lsf-checkout, for a store with one product
    /// (4 at 32.00 in the cart) and the given discount, and says whether it exits with 1.
    fn run_fails(discount: Json, handle: &str, function_settings: Option<Json>) -> bool {
        let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../checkout/tests/fixtures/app");
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
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../checkout/tests/fixtures/app");
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
            app: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../checkout/tests/fixtures/app")),
            build: false,
            session: None,
            scenario: scenario.map(str::to_string),
            checkout: None,
            locale: None,
            step: Step::Completion,
            format: Format::Json,
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
                "functions": {"quantity-limit": {"metafields": {"$app:config.settings": {"type": "json", "value": {"maxQuantity": 3}}}}}
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
