//! `lsf checkout`: serves the checkout of an app, or with `init` writes the scenarios to try it
//! with.

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
}

pub fn run(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_core::Preview],
    args: CheckoutArgs,
) -> Result<ExitCode, String> {
    match args.command {
        Some(Subcommand::Init(init)) => checkout_init::run(theme, data, init),
        None => serve::run(theme, data, previews, args.serve, true),
    }
}
