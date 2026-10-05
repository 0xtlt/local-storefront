//! `lsf`: serve a Shopify theme locally, rendered from JSON fixtures instead of the Shopify API.

mod app;
mod commands;
mod output;
mod profile;
mod server;

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
                  `lsf init` writes that demo store to disk as a starting point."
)]
struct Cli {
    /// The theme directory.
    #[arg(long, global = true, env = "LSF_THEME", default_value = ".")]
    theme: PathBuf,

    /// The store data directory. Defaults to <theme>/shopify-local, or the built-in demo store.
    #[arg(long, global = true, env = "LSF_DATA")]
    data: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the theme over HTTP.
    Serve(commands::serve::Args),
    /// Render one URL and print the HTML.
    Render(commands::render::Args),
    /// Render one URL and print what the render spent its time in: sections, blocks, snippets.
    Profile(commands::profile::Args),
    /// Check the store data and explain every problem found.
    Validate(commands::validate::Args),
    /// Create the data directory: a demo store, JSON Schemas for editors, and a guide.
    Init(commands::init::Args),
    /// Print the JSON Schema of the store data format.
    Schema(commands::schema::Args),
    /// List the URLs the store data gives a page to.
    Routes,
    /// Parse every Liquid file of the theme and report what would not render on Shopify.
    Check,
    /// Print the reference of the store data format.
    Docs(commands::docs::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Serve(args) => commands::serve::run(&cli.theme, cli.data.as_deref(), args),
        Command::Render(args) => commands::render::run(&cli.theme, cli.data.as_deref(), args),
        Command::Profile(args) => commands::profile::run(&cli.theme, cli.data.as_deref(), args),
        Command::Validate(args) => commands::validate::run(&cli.theme, cli.data.as_deref(), args),
        Command::Init(args) => commands::init::run(&cli.theme, cli.data.as_deref(), args),
        Command::Schema(args) => commands::schema::run(args),
        Command::Routes => commands::routes::run(&cli.theme, cli.data.as_deref()),
        Command::Check => commands::check::run(&cli.theme),
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
