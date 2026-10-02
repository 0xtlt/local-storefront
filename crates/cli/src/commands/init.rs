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

pub fn run(theme: &Path, data: Option<&Path>, args: Args) -> Result<ExitCode, String> {
    let directory = match data {
        Some(directory) => directory.to_path_buf(),
        None => theme.join(DEFAULT_DATA_DIRECTORY),
    };
    let write = |relative: &str, content: &str| -> Result<bool, String> {
        let path = directory.join(relative);
        if path.exists() && !args.force {
            return Ok(false);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        std::fs::write(&path, content).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(true)
    };

    let mut written = 0;
    let mut skipped = 0;
    for (name, content) in demo::FILES {
        if write(name, content)? {
            written += 1;
        } else {
            skipped += 1;
        }
    }
    if write("README.md", &docs::guide())? {
        written += 1;
    } else {
        skipped += 1;
    }
    write("files/.gitkeep", "")?;
    schema::write_all(&directory.join("schema"))?;

    eprintln!(
        "{}: wrote {written} file(s), kept {skipped} existing one(s), refreshed schema/",
        directory.display()
    );
    eprintln!("next: `lsf serve` to see the theme, `lsf validate` after editing the data");
    Ok(ExitCode::SUCCESS)
}
