use std::path::PathBuf;
use std::process::ExitCode;

use lsf_core::store::validate::{FileKind, schema};

#[derive(clap::Args)]
pub struct Args {
    /// Which schema: store (a root data file), product, collection, page, blog, customer,
    /// menu or session.
    #[arg(default_value = "store")]
    kind: String,

    /// Write every schema into this directory as <kind>.schema.json instead of printing one.
    #[arg(long)]
    out: Option<PathBuf>,
}

/// Writes all the schemas into a directory.
pub fn write_all(directory: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    for kind in FileKind::ALL {
        let path = directory.join(format!("{}.schema.json", kind.name()));
        let content =
            serde_json::to_string_pretty(&schema(kind)).map_err(|error| error.to_string())?;
        std::fs::write(&path, content + "\n")
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}

pub fn run(args: Args) -> Result<ExitCode, String> {
    if let Some(directory) = args.out {
        write_all(&directory)?;
        eprintln!(
            "wrote {} schemas to {}",
            FileKind::ALL.len(),
            directory.display()
        );
        return Ok(ExitCode::SUCCESS);
    }
    let kind = FileKind::ALL
        .into_iter()
        .find(|kind| kind.name() == args.kind)
        .ok_or_else(|| {
            let names: Vec<&str> = FileKind::ALL.iter().map(|kind| kind.name()).collect();
            format!(
                "unknown schema \"{}\". Available: {}",
                args.kind,
                names.join(", ")
            )
        })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&schema(kind)).map_err(|error| error.to_string())?
    );
    Ok(ExitCode::SUCCESS)
}
