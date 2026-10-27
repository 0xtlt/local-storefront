use std::path::Path;
use std::process::ExitCode;

use lsf_checkout::app::Selection;
use lsf_checkout::pipeline;
use lsf_checkout::{BuyerJourneyStep, CheckoutResult, CheckoutState};
use lsf_core::Session;
use lsf_core::diagnostics::Diagnostics;
use lsf_core::store::model::SessionInput;
use lsf_core::store::validate::{FileKind, validate};
use serde_json::Value as Json;

use crate::app::App;
use crate::checkout::{self, config_from_store};
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
    let Subcommand::Run(args) = args.command;
    let (shopify_app, result) = compute(theme, data, previews, &args)?;
    let failed = result.runs.iter().any(|run| run.error.is_some());

    match args.format {
        Format::Json => {
            let mut json = serde_json::to_value(&result).map_err(|error| error.to_string())?;
            json["diagnostics"] = Json::Array(
                shopify_app
                    .diagnostics
                    .iter()
                    .map(|diagnostic| Json::String(diagnostic.to_string()))
                    .collect(),
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&json).map_err(|error| error.to_string())?
            );
        }
        Format::Text => print_text(&result),
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Builds the input of each Function from the store data and runs them, for the step, the
/// language and the checkout the arguments say.
fn compute(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_core::Preview],
    args: &RunArgs,
) -> Result<(lsf_checkout::app::App, CheckoutResult), String> {
    let Some(app_dir) = &args.app else {
        return Err("`lsf function run` needs the app: `--app DIR` (or LSF_APP)".to_string());
    };
    let app_args = checkout::AppArgs {
        app: args.app.clone(),
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

    let local = App::open_without_theme(theme, data, previews)?;
    let (store, diagnostics) = local.load_store();
    print_diagnostics(&diagnostics);
    if diagnostics.has_errors() {
        return Err("the store data has errors (see above)".to_string());
    }

    let mut session = Session::initial(&store);
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
    let mut state: CheckoutState = match &args.checkout {
        Some(value) => serde_json::from_value(read_json("checkout", value)?)
            .map_err(|error| format!("--checkout: {error}"))?,
        None => CheckoutState::default(),
    };
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

    let config = config_from_store(&store);
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
    Ok((shopify_app, result))
}

fn money(cents: i64, currency: &str) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    format!(
        "{sign}{}.{:02} {currency}",
        cents.abs() / 100,
        cents.abs() % 100
    )
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
            println!("  {} × {title}  {}", line.quantity, money(total, currency));
        } else {
            println!(
                "  {} × {title}  {} → {}",
                line.quantity,
                money(subtotal, currency),
                money(total, currency)
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
            money(discount.amount, currency),
            discount.class,
            discount.function
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
                money(option.cost, currency)
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
        money(cost.subtotal, currency),
        money(cost.discounts, currency),
        money(cost.shipping, currency),
        money(cost.tax, currency),
        money(cost.total, currency)
    );
}

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
                checkout: None,
                locale: None,
                step,
                format: Format::Json,
            };
            let (_, result) = compute(&root, Some(&data), &[], &args).unwrap();
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
    fn the_step_defaults_to_the_completion() {
        #[derive(clap::Parser)]
        struct Cli {
            #[command(flatten)]
            args: Args,
        }
        let step = |extra: &[&str]| {
            let mut words = vec!["lsf", "run", "quantity-limit"];
            words.extend_from_slice(extra);
            let Subcommand::Run(run) = Cli::parse_from(words).args.command;
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

    #[test]
    fn money_prints_with_its_currency() {
        assert_eq!(money(1999, "EUR"), "19.99 EUR");
        assert_eq!(money(-5, "USD"), "-0.05 USD");
        assert_eq!(money(0, "USD"), "0.00 USD");
    }
}
