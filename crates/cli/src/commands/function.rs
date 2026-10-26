use std::path::Path;
use std::process::ExitCode;

use lsf_checkout::app::Selection;
use lsf_checkout::pipeline;
use lsf_checkout::{CheckoutResult, CheckoutState};
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
    let state: CheckoutState = match &args.checkout {
        Some(value) => serde_json::from_value(read_json("checkout", value)?)
            .map_err(|error| format!("--checkout: {error}"))?,
        None => CheckoutState::default(),
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
    fn money_prints_with_its_currency() {
        assert_eq!(money(1999, "EUR"), "19.99 EUR");
        assert_eq!(money(-5, "USD"), "-0.05 USD");
        assert_eq!(money(0, "USD"), "0.00 USD");
    }
}
