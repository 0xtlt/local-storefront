use std::path::Path;
use std::process::ExitCode;

use lsf_core::store::demo;

use crate::app::DEFAULT_DATA_DIRECTORY;
use crate::commands::{docs, schema};

#[derive(clap::Args)]
pub struct Args {
    /// Overwrite files that already exist.
    #[arg(long)]
    force: bool,
}

/// The data directory `init` and `app init` write to: `--data`, otherwise
/// `<theme>/shopify-local`.
pub fn data_directory(theme: &Path, data: Option<&Path>) -> std::path::PathBuf {
    match data {
        Some(directory) => directory.to_path_buf(),
        None => theme.join(DEFAULT_DATA_DIRECTORY),
    }
}

/// Writes `content` to `relative` in `directory`, creating the folders it needs. A file that
/// exists is left alone unless `force`. Says whether the file was written.
pub fn write_file(
    directory: &Path,
    relative: &str,
    content: &str,
    force: bool,
) -> Result<bool, String> {
    let path = directory.join(relative);
    if path.exists() && !force {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(&path, content).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(true)
}

/// Writes the demo store, its guide and the keep-file of `files/`. Returns how many files were
/// written and how many were kept because they exist.
pub fn write_demo_store(directory: &Path, force: bool) -> Result<(usize, usize), String> {
    let mut written = 0;
    let mut skipped = 0;
    let mut count = |wrote: bool| {
        if wrote {
            written += 1;
        } else {
            skipped += 1;
        }
    };
    for (name, content) in demo::FILES {
        count(write_file(directory, name, content, force)?);
    }
    count(write_file(directory, "README.md", &docs::guide(), force)?);
    write_file(directory, "files/.gitkeep", "", force)?;
    Ok((written, skipped))
}

pub fn run(theme: &Path, data: Option<&Path>, args: Args) -> Result<ExitCode, String> {
    let directory = data_directory(theme, data);
    let (written, skipped) = write_demo_store(&directory, args.force)?;
    schema::write_all(&directory.join("schema"))?;

    eprintln!(
        "{}: wrote {written} file(s), kept {skipped} existing one(s), refreshed schema/",
        directory.display()
    );
    eprintln!(
        "next: the `serve` command shows the theme, and `validate` checks the data after you edit it"
    );
    eprintln!(
        "      for a Shopify app: `app init --app <dir>` writes checkout scenarios to try its Functions and extensions with"
    );
    Ok(ExitCode::SUCCESS)
}
