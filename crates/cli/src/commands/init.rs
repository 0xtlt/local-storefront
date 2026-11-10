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
        "{}: wrote {}, kept {}, refreshed schema/",
        directory.display(),
        crate::output::count(written, "file", "files"),
        crate::output::count(skipped, "existing one", "existing ones")
    );
    eprintln!(
        "next: the `serve` command shows the theme, and `validate` checks the data after you edit it"
    );
    eprintln!(
        "      for a Shopify app: `app init --app <dir>` writes checkout scenarios to try its Functions and extensions with"
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guide's links to the docs of a version (`/blob/v0.2.0/docs/`) as `/blob/v*/docs/`: a
    /// copy an earlier version wrote is current as long as the rest is the same.
    fn any_version(guide: &str) -> String {
        const MARK: &str = "/blob/v";
        let mut out = String::with_capacity(guide.len());
        let mut rest = guide;
        while let Some(at) = rest.find(MARK) {
            out.push_str(&rest[..at + MARK.len()]);
            out.push('*');
            rest = &rest[at + MARK.len()..];
            rest = &rest[rest.find('/').unwrap_or(rest.len())..];
        }
        out.push_str(rest);
        out
    }

    fn file_names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// The store data of `examples/checkout-playground` keeps the guide and the schemas
    /// `lsf init` writes, for the readers of the example and their editors.
    /// `UPDATE_SNAPSHOTS=1 cargo test -p lsf-cli the_example_data` writes them again.
    #[test]
    fn the_example_data_has_the_guide_and_the_schemas_init_writes() {
        let example = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/checkout-playground/shopify-local");
        if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
            write_file(&example, "README.md", &docs::guide(), true).expect("write the guide");
            let _ = std::fs::remove_dir_all(example.join("schema"));
            schema::write_all(&example.join("schema")).expect("write the schemas");
            return;
        }
        let stale = "is not what `lsf init` writes: run \
            `UPDATE_SNAPSHOTS=1 cargo test -p lsf-cli the_example_data`";
        let readme = std::fs::read_to_string(example.join("README.md")).unwrap_or_default();
        assert!(
            any_version(&readme) == any_version(&docs::guide()),
            "examples/checkout-playground/shopify-local/README.md {stale}"
        );
        let fresh = std::env::temp_dir().join(format!("lsf-init-schemas-{}", std::process::id()));
        schema::write_all(&fresh).expect("write the schemas");
        assert_eq!(
            file_names(&example.join("schema")),
            file_names(&fresh),
            "examples/checkout-playground/shopify-local/schema/ {stale}"
        );
        for name in file_names(&fresh) {
            let read = |directory: &Path| std::fs::read_to_string(directory.join(&name)).ok();
            assert!(
                read(&example.join("schema")) == read(&fresh),
                "examples/checkout-playground/shopify-local/schema/{name} {stale}"
            );
        }
        let _ = std::fs::remove_dir_all(&fresh);
    }

    #[test]
    fn any_version_leaves_the_rest_of_a_link() {
        assert_eq!(
            any_version("[a](https://github.com/o/r/blob/v0.2.0/docs/a.md#x) [b](b.md)"),
            "[a](https://github.com/o/r/blob/v*/docs/a.md#x) [b](b.md)"
        );
    }
}
