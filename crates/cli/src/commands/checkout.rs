//! `lsf checkout`: serves the checkout of an app, with `init` writes the scenarios to try it
//! with, and with `build` builds its extensions.

use std::path::Path;
use std::process::ExitCode;

use crate::commands::{checkout_init, serve};

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct CheckoutArgs {
    #[command(flatten)]
    serve: serve::Args,

    #[command(subcommand)]
    command: Option<Subcommand>,
}

#[derive(clap::Subcommand)]
enum Subcommand {
    /// Write checkout scenarios to the `checkouts` folder of the store data, made for the
    /// Functions and checkout UI extensions of `--app` (a generic set without it), and create the
    /// data folder with the demo store when it does not exist. Nothing that exists is
    /// overwritten without `--force`.
    Init(checkout_init::InitArgs),
    /// Build the extensions of an app, as `shopify app build` does: its Functions (as
    /// `lsf function build`), and its checkout UI extensions that name their sources (`module`),
    /// bundled by lsf without Node (`dist/<handle>.js`).
    Build(BuildArgs),
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
    args: CheckoutArgs,
) -> Result<ExitCode, String> {
    match args.command {
        Some(Subcommand::Init(init)) => checkout_init::run(theme, data, init),
        Some(Subcommand::Build(args)) => crate::checkout::run_build(
            args.app.as_deref(),
            &args.handles,
            "lsf checkout build",
            crate::checkout::build,
        ),
        None => serve::run(theme, data, previews, args.serve, true),
    }
}
