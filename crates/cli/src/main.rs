//! `lsf`: serve a Shopify theme locally, rendered from JSON fixtures instead of the Shopify API.

mod app;
mod checkout;
mod commands;
mod output;
mod profile;
mod server;
mod watch;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

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
    previews: Vec<lsf_theme::Preview>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the theme over HTTP. With `--app`, its `/checkout` runs the Functions and shows the
    /// checkout UI extensions of a Shopify app, its `/account` shows its customer account UI
    /// extensions, and a local Admin API serves the app's backend.
    Serve(commands::serve::Args),
    /// The commands of a Shopify app: `dev` serves it alone (its checkout, its customer account
    /// and the Admin API of its backend), `init` writes checkout scenarios to try it with, `build`
    /// builds its extensions, `check` runs the scenarios for CI, `function run`, `function build`
    /// and `function schema` its Functions, and `webhook trigger` sends a webhook to its backend.
    App(commands::app::Args),
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

fn parse_preview(name: &str) -> Result<lsf_theme::Preview, String> {
    lsf_theme::Preview::from_name(name).ok_or_else(|| {
        let known: Vec<&str> = lsf_theme::Preview::ALL
            .iter()
            .map(|preview| preview.name())
            .collect();
        format!("no preview is called `{name}`; known: {}", known.join(", "))
    })
}

/// Where the options of `serve` or `app dev` got their values, when one of them runs.
fn given(matches: &clap::ArgMatches) -> commands::serve::Given {
    let serve = matches.subcommand_matches("serve").or_else(|| {
        matches
            .subcommand_matches("app")
            .and_then(|app| app.subcommand_matches("dev"))
    });
    serve.map(commands::serve::Given::of).unwrap_or_default()
}

/// A command that only prints, piped into a reader that stops early (`lsf docs | head`), ends
/// quietly as `cat` does, instead of panicking on the closed pipe. Only for those: a server or
/// a client keeps the closed socket of a peer an error to answer, not a signal that kills it.
fn end_quietly_on_a_closed_pipe(command: &Command) {
    let prints_only = matches!(
        command,
        Command::Render(_)
            | Command::Profile(_)
            | Command::Validate(_)
            | Command::Schema(_)
            | Command::Routes
            | Command::Check(_)
            | Command::Fmt(_)
            | Command::Docs(_)
    );
    #[cfg(unix)]
    if prints_only {
        // SAFETY: at the start of `main`, before any thread; SIG_DFL is the default action.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }
    #[cfg(not(unix))]
    let _ = prints_only;
}

fn main() -> ExitCode {
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    end_quietly_on_a_closed_pipe(&cli.command);
    let given = given(&matches);
    let previews = cli.previews.as_slice();
    let result = match cli.command {
        Command::Serve(args) => commands::serve::run(
            &cli.theme,
            cli.data.as_deref(),
            previews,
            args,
            &given,
            false,
        ),
        Command::App(args) => {
            commands::app::run(&cli.theme, cli.data.as_deref(), previews, args, &given)
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
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_help_of_serve_lists_the_options_of_the_theme_first() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("serve")
            .expect("serve")
            .render_help()
            .to_string();
        let at = |text: &str| help.find(text).unwrap_or_else(|| panic!("{text}: {help}"));
        let app = at("App (with --app):");
        let admin = at("Admin API and embedded admin (with --app):");
        for theme in [
            "--live-reload",
            "--port",
            "--static",
            "--throttle",
            "--customer",
        ] {
            assert!(at(theme) < app, "{theme}");
        }
        for of_app in ["--app <DIR>", "--watch", "--variant", "--app-url"] {
            assert!(app < at(of_app) && at(of_app) < admin, "{of_app}");
        }
        for of_admin in ["--admin-throttle", "--pricing-plans", "--polaris"] {
            assert!(admin < at(of_admin), "{of_admin}");
        }
    }

    #[test]
    fn the_commands_of_an_app_are_under_app() {
        let parse = |words: &[&str]| {
            Cli::try_parse_from(std::iter::once("lsf").chain(words.iter().copied()))
        };
        assert!(matches!(
            parse(&["app", "dev", "--app", "../my-app", "--port", "0"])
                .unwrap()
                .command,
            Command::App(_)
        ));
        assert!(parse(&["app", "init", "--app", "../my-app", "--force"]).is_ok());
        assert!(parse(&["--data", "d", "app", "init"]).is_ok());
        assert!(parse(&["app", "build", "banner", "--app", "../my-app"]).is_ok());
        assert!(parse(&["app", "check", "--format", "json"]).is_ok());
        assert!(parse(&["app", "function", "run", "discount", "--scenario", "vip"]).is_ok());
        assert!(parse(&["app", "function", "build", "discount"]).is_ok());
        assert!(parse(&["app", "webhook", "trigger", "--topic", "orders/create"]).is_ok());
        assert!(
            parse(&[
                "app", "webhook", "trigger", "--path", "app", "--topic", "x", "--json"
            ])
            .is_ok()
        );
        assert!(parse(&["app", "webhook", "trigger"]).is_err());
        // `app` alone does nothing, and the options of `dev` are its own.
        assert!(parse(&["app"]).is_err());
        assert!(parse(&["app", "--port", "0"]).is_err());
        assert!(parse(&["app", "init", "--port", "0"]).is_err());
        // The old names are gone.
        assert!(parse(&["checkout", "--app", "../my-app"]).is_err());
        assert!(parse(&["function", "run"]).is_err());
    }
}
