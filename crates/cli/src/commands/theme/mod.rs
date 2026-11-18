//! `lsf theme`: the commands about the theme alone, without the store data.

pub mod digest;

use std::path::Path;
use std::process::ExitCode;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Print one digest of the theme, `sha256:<hex>  <n> files`: the SHA-256 of the lines
    /// `<md5>  <path>` of its files, sorted by path. Two themes with the same digest hold the
    /// same files, as Shopify CLI compares them. `file` prints the lines of the files given.
    ///
    /// The files are those of the theme's folders (assets, blocks, config, layout, locales,
    /// sections, snippets, templates), without `assets/x` when `assets/x.liquid` exists (Shopify
    /// generates it). Code only: `--ignore 'templates/**/*.json' --ignore 'sections/*.json'
    /// --ignore 'config/settings_data.json' --ignore 'locales/*.json'`.
    Digest(digest::Args),
}

pub fn run(theme: &Path, args: Args) -> Result<ExitCode, String> {
    match args.command {
        Command::Digest(args) => digest::run(theme, args),
    }
}
