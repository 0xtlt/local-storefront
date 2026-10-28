//! `lsf`: serve a Shopify theme locally, rendered from JSON fixtures instead of the Shopify API.

mod app;
mod checkout;
mod commands;
mod output;
mod profile;
mod server;
mod shopify_cli;
mod watch;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

// Rendering a page makes hundreds of thousands of small allocations. The system allocators
// (macOS's and musl's in particular) make threads that allocate at the same time slow each
// other down, which caps how many pages the server renders in parallel.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(
    name = "lsf",
    version,
    about = "Serve a Shopify theme locally, rendered from JSON fixtures instead of the Shopify API",
    long_about = "Serve a Shopify theme locally, rendered from JSON fixtures instead of the Shopify API.\n\n\
                  Run it in a theme directory (the one with layout/, sections/, templates/, ...). The store \
                  data is read from ./shopify-local when it exists, otherwise a built-in demo store is used; \
                  the `init` command writes that demo store to disk as a starting point."
)]
struct Cli {
    /// The theme directory.
    #[arg(long, global = true, env = "LSF_THEME", default_value = ".")]
    theme: PathBuf,

    /// The store data directory. Defaults to <theme>/shopify-local, or the built-in demo store.
    #[arg(long, global = true, env = "LSF_DATA")]
    data: Option<PathBuf>,

    /// Turns on a feature preview of Shopify, as a development store can: `liquid-july-26`
    /// ({% block %}, {% partial %} and @shopify/partial-rendering). Without it, lsf renders
    /// what a live store renders. Repeat it, or separate names with commas.
    #[arg(
        long = "preview",
        global = true,
        env = "LSF_PREVIEW",
        value_delimiter = ',',
        value_name = "NAME",
        value_parser = parse_preview
    )]
    previews: Vec<lsf_core::Preview>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the theme over HTTP. With `--app`, its `/checkout` runs the Functions and shows the
    /// checkout UI extensions of a Shopify app.
    Serve(commands::serve::Args),
    /// Serve the checkout of a Shopify app alone, without a theme: `serve --app` without the
    /// storefront. It takes the options of `serve`. `checkout init` writes the checkout
    /// scenarios to try the app with.
    Checkout(commands::checkout::CheckoutArgs),
    /// Run the Shopify Functions of an app against the store data.
    Function(commands::function::Args),
    /// Render one URL and print the HTML.
    Render(commands::render::Args),
    /// Render one URL and print what the render spent its time in: sections, blocks, snippets.
    Profile(commands::profile::Args),
    /// Check the store data and explain every problem found, checkout scenarios included. Runs in
    /// a theme, or in an app folder with a shopify-local/ directory and no theme.
    Validate(commands::validate::Args),
    /// Create the data directory: a demo store, JSON Schemas for editors, and a guide.
    Init(commands::init::Args),
    /// Print the JSON Schema of the store data format.
    Schema(commands::schema::Args),
    /// List the URLs the store data gives a page to.
    Routes,
    /// Parse every Liquid file of the theme and report what would not render on Shopify,
    /// including what Shopify's strict Liquid parser rejects. `--fix` rewrites that markup.
    Check(commands::check::Args),
    /// Format the Liquid files of the theme, as Prettier does with Shopify's Liquid plugin.
    Fmt(commands::fmt::Args),
    /// Print the reference of the store data format.
    Docs(commands::docs::Args),
}

fn parse_preview(name: &str) -> Result<lsf_core::Preview, String> {
    lsf_core::Preview::from_name(name).ok_or_else(|| {
        let known: Vec<&str> = lsf_core::Preview::ALL
            .iter()
            .map(|preview| preview.name())
            .collect();
        format!("no preview is called `{name}`; known: {}", known.join(", "))
    })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let previews = cli.previews.as_slice();
    let result = match cli.command {
        Command::Serve(args) => {
            commands::serve::run(&cli.theme, cli.data.as_deref(), previews, args, false)
        }
        Command::Checkout(args) => {
            commands::checkout::run(&cli.theme, cli.data.as_deref(), previews, args)
        }
        Command::Function(args) => {
            commands::function::run(&cli.theme, cli.data.as_deref(), previews, args)
        }
        Command::Render(args) => {
            commands::render::run(&cli.theme, cli.data.as_deref(), previews, args)
        }
        Command::Profile(args) => {
            commands::profile::run(&cli.theme, cli.data.as_deref(), previews, args)
        }
        Command::Validate(args) => {
            commands::validate::run(&cli.theme, cli.data.as_deref(), previews, args)
        }
        Command::Init(args) => commands::init::run(&cli.theme, cli.data.as_deref(), args),
        Command::Schema(args) => commands::schema::run(args),
        Command::Routes => commands::routes::run(&cli.theme, cli.data.as_deref(), previews),
        Command::Check(args) => commands::check::run(&cli.theme, previews, args),
        Command::Fmt(args) => commands::fmt::run(&cli.theme, args),
        Command::Docs(args) => commands::docs::run(args),
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn checkout_serves_and_checkout_init_writes_scenarios() {
        let parse = |words: &[&str]| {
            Cli::try_parse_from(std::iter::once("lsf").chain(words.iter().copied()))
        };
        assert!(matches!(
            parse(&["checkout", "--app", "../my-app", "--port", "0"])
                .unwrap()
                .command,
            Command::Checkout(_)
        ));
        assert!(parse(&["checkout", "init", "--app", "../my-app", "--force"]).is_ok());
        assert!(parse(&["--data", "d", "checkout", "init"]).is_ok());
        // Serving and initialising are not combined.
        assert!(parse(&["checkout", "--port", "0", "init"]).is_err());
        assert!(parse(&["checkout", "init", "--port", "0"]).is_err());
    }
}
