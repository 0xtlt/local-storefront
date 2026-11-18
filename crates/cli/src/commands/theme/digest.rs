//! `lsf theme digest`: one digest of the whole theme, and with `file`, the checksum Shopify CLI
//! computes for each file given, to tell whether two copies of a theme hold the same files.
//!
//! A checksum is [`lsf_core::theme::checksum::checksum_md5`]. The digest of a theme is the
//! SHA-256 of the lines `<md5>  <path>\n` of its files, sorted by path.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lsf_core::theme::THEME_DIRECTORIES;
use lsf_core::theme::checksum::checksum_md5;
use sha2::{Digest, Sha256};

use crate::commands::fmt::glob_match;

/// Without a subcommand, the digest of the theme; its options are refused with `file`.
#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Args {
    #[command(subcommand)]
    file: Option<Command>,

    /// Keeps only the files whose path matches one of these globs (`sections/*.liquid`; `**`
    /// crosses folders). Repeat it.
    #[arg(long = "include", value_name = "GLOB")]
    includes: Vec<String>,

    /// Leaves out the files whose path matches one of these globs, after `--include`. Repeat it.
    #[arg(long = "ignore", value_name = "GLOB")]
    ignores: Vec<String>,

    /// Prints the lines that were hashed before the digest, to see what the globs kept.
    #[arg(long)]
    list: bool,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Print the checksum Shopify CLI computes for each file to compare it with a store's
    /// `checksumMd5`, as `<md5>  <path>`, the path relative to the theme.
    File(FileArgs),
}

#[derive(clap::Args)]
struct FileArgs {
    /// The files, in the theme.
    #[arg(required = true)]
    paths: Vec<PathBuf>,
}

/// The names Shopify CLI never pushes (its `DEFAULT_IGNORE_PATTERNS`): left out of a theme's
/// digest, with what is under them.
const NEVER_PUSHED: [&str; 13] = [
    ".git",
    ".vscode",
    ".hg",
    ".bzr",
    ".svn",
    "_darcs",
    "CVS",
    ".DS_Store",
    ".sass-cache",
    "Thumbs.db",
    "desktop.ini",
    "config.yml",
    "node_modules",
];

pub fn run(theme: &Path, args: Args) -> Result<ExitCode, String> {
    let root = theme_root(theme)?;
    match args.file {
        Some(Command::File(args)) => {
            let mut lines = String::new();
            for path in &args.paths {
                let key = key_of(&root, path)?;
                let bytes = fs::read(path)
                    .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
                lines.push_str(&line(&key, &bytes));
            }
            print!("{lines}");
        }
        None => {
            let files = theme_files(&root)?;
            let lines: String = kept(&files, &args.includes, &args.ignores)
                .map(|(key, bytes)| line(key, bytes))
                .collect();
            if args.list {
                print!("{lines}");
            }
            println!("{}", summary(&lines));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The theme folder, made canonical, refused as `lsf serve` refuses it.
fn theme_root(theme: &Path) -> Result<PathBuf, String> {
    if !theme.exists() {
        return Err(format!(
            "the theme folder {} does not exist: give `--theme` the folder of a theme (the one with layout/ and templates/)",
            theme.display()
        ));
    }
    if !theme.is_dir() {
        return Err(format!(
            "{} is a file, not a folder: give `--theme` the folder of a theme (the one with layout/ and templates/)",
            theme.display()
        ));
    }
    lsf_core::util::canonicalize(theme).map_err(|error| format!("{}: {error}", theme.display()))
}

/// The line of a file: its checksum, two spaces and its path, as `md5sum` prints it.
fn line(key: &str, bytes: &[u8]) -> String {
    format!("{}  {key}\n", checksum_md5(key, bytes))
}

/// The digest of the lines of a theme, and how many files they are.
fn summary(lines: &str) -> String {
    let files = lines.lines().count();
    let hex: String = Sha256::digest(lines.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let noun = if files == 1 { "file" } else { "files" };
    format!("sha256:{hex}  {files} {noun}")
}

/// The path of a file relative to the theme, with `/`: what Shopify names it. The folder of
/// the file is made canonical, not the file, so that a link in the theme keeps its name.
fn key_of(root: &Path, path: &Path) -> Result<String, String> {
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(format!("{} is not a file", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("{} does not exist", path.display()));
        }
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    }
    let outside = || {
        format!(
            "{} is not in the theme {}: give a file of the theme, or set `--theme`",
            path.display(),
            root.display()
        )
    };
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    let name = absolute.file_name().ok_or_else(outside)?;
    let folder = lsf_core::util::canonicalize(absolute.parent().ok_or_else(outside)?)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let relative = folder.join(name);
    let relative = relative.strip_prefix(root).map_err(|_| outside())?;
    let parts: Vec<String> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    Ok(parts.join("/"))
}

/// The files of the theme's folders, by path: their bytes. Links to folders are not followed.
fn theme_files(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    fn walk(
        directory: &Path,
        key: &str,
        files: &mut BTreeMap<String, Vec<u8>>,
    ) -> Result<(), String> {
        let entries = fs::read_dir(directory)
            .map_err(|error| format!("{} cannot be read: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_sublime =
                name.ends_with(".sublime-project") || name.ends_with(".sublime-workspace");
            if NEVER_PUSHED.contains(&name.as_str()) || is_sublime {
                continue;
            }
            let path = entry.path();
            let child = format!("{key}/{name}");
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_dir() {
                walk(&path, &child, files)?;
            } else if path.is_file() {
                let bytes = fs::read(&path)
                    .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
                files.insert(child, bytes);
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    for directory in THEME_DIRECTORIES {
        let path = root.join(directory);
        if path.is_dir() {
            walk(&path, directory, &mut files)?;
        }
    }
    Ok(files)
}

/// The files of a theme its digest covers, in the order of their paths: without the assets
/// Shopify generates from a Liquid one (`assets/x` beside `assets/x.liquid`, as Shopify CLI's
/// `rejectGeneratedStaticAssets`), then those the globs keep.
fn kept<'a>(
    files: &'a BTreeMap<String, Vec<u8>>,
    includes: &'a [String],
    ignores: &'a [String],
) -> impl Iterator<Item = (&'a String, &'a Vec<u8>)> {
    files.iter().filter(move |(key, _)| {
        let generated = key.starts_with("assets/") && files.contains_key(&format!("{key}.liquid"));
        let included = includes.is_empty() || includes.iter().any(|glob| glob_match(glob, key));
        let ignored = ignores.iter().any(|glob| glob_match(glob, key));
        !generated && included && !ignored
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(files: &[&str], includes: &[&str], ignores: &[&str]) -> Vec<String> {
        let files: BTreeMap<String, Vec<u8>> = files
            .iter()
            .map(|key| (key.to_string(), Vec::new()))
            .collect();
        let includes: Vec<String> = includes.iter().map(|glob| glob.to_string()).collect();
        let ignores: Vec<String> = ignores.iter().map(|glob| glob.to_string()).collect();
        kept(&files, &includes, &ignores)
            .map(|(key, _)| key.clone())
            .collect()
    }

    #[test]
    fn a_generated_asset_is_left_out_as_shopify_cli_leaves_it() {
        // `rejectGeneratedStaticAssets`'s own test.
        assert_eq!(
            keys(
                &[
                    "assets/basic.css",
                    "assets/basic.css.liquid",
                    "assets/complex.css",
                    "assets/image.png",
                    "config/settings_data.json",
                    "config/settings_schema.json",
                    "sections/announcement-bar.liquid",
                    // Only an asset is generated.
                    "snippets/a",
                    "snippets/a.liquid",
                ],
                &[],
                &[]
            ),
            [
                "assets/basic.css.liquid",
                "assets/complex.css",
                "assets/image.png",
                "config/settings_data.json",
                "config/settings_schema.json",
                "sections/announcement-bar.liquid",
                "snippets/a",
                "snippets/a.liquid",
            ]
        );
    }

    #[test]
    fn includes_keep_and_ignores_then_leave_out() {
        let files = [
            "config/settings_data.json",
            "config/settings_schema.json",
            "sections/header.liquid",
            "sections/header-group.json",
            "templates/index.json",
            "templates/customers/account.json",
            "templates/page.liquid",
        ];
        assert_eq!(keys(&files, &[], &[]).len(), files.len());
        assert_eq!(
            keys(&files, &[], &["templates/**/*.json", "sections/*.json"]),
            [
                "config/settings_data.json",
                "config/settings_schema.json",
                "sections/header.liquid",
                "templates/page.liquid",
            ]
        );
        assert_eq!(
            keys(
                &files,
                &["config/**", "templates/**"],
                &["config/settings_data.json"]
            ),
            [
                "config/settings_schema.json",
                "templates/customers/account.json",
                "templates/index.json",
                "templates/page.liquid",
            ]
        );
        assert!(keys(&files, &["nothing/**"], &[]).is_empty());
    }

    #[test]
    fn the_digest_hashes_the_lines() {
        assert_eq!(
            summary(""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  0 files"
        );
        let lines = line("snippets/a.liquid", b"a\r\nb\r\n");
        assert_eq!(
            lines,
            "dd8c6a395b5dd36c56d23275028f526c  snippets/a.liquid\n"
        );
        let hex: String = Sha256::digest(lines.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(summary(&lines), format!("sha256:{hex}  1 file"));
    }
}
