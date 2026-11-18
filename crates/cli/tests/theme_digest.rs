//! `lsf theme digest`: the digest of a theme and the checksum of its files (`file`), in a theme of the
//! test's own. The checksums are those Shopify CLI computes (`asset-checksum.ts`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

/// A theme of the test's, removed with the value.
struct Theme(PathBuf);

impl Theme {
    fn new(name: &str) -> Theme {
        let path = std::env::temp_dir().join(format!("lsf-digest-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let theme = Theme(path);
        theme.write(
            "layout/theme.liquid",
            b"<html>{{ content_for_layout }}</html>\n",
        );
        theme.write("snippets/crlf.liquid", b"a\r\nb\r\n");
        theme.write("assets/crlf.png", b"a\r\nb\r\n");
        theme.write("assets/basic.css.liquid", b"a\r\nb\r\n");
        theme.write("assets/basic.css", b"generated");
        theme.write(
            "config/settings_data.json",
            b"/* c */\r\n{ \"a\": \"x  y\" }\r\n",
        );
        theme.write(
            "templates/index.json",
            b"{ \"sections\": {}, \"order\": [] }",
        );
        theme.write("templates/customers/account.json", b"{}");
        theme.write("sections/header-group.json", b"{}");
        // Outside the theme's folders, or never pushed: not in its digest.
        theme.write("README.md", b"# theme\n");
        theme.write("assets/.DS_Store", b"finder");
        theme
    }

    fn write(&self, file: &str, bytes: &[u8]) {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

impl Drop for Theme {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn lsf(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lsf"))
        .args(args)
        .current_dir(directory)
        .env_remove("LSF_THEME")
        .env_remove("LSF_DATA")
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

const CRLF_LIQUID: &str = "dd8c6a395b5dd36c56d23275028f526c";
const RAW_CRLF: &str = "59b0d7772f0561efb95518f3cb8abc60";
/// The MD5 of `{"a":"x  y"}`, the settings minified.
const SETTINGS: &str = "985a8b9f94fe878923016b6abdcdbcdf";

#[test]
fn a_file_has_the_checksum_of_shopify_cli_and_its_path_in_the_theme() {
    let theme = Theme::new("file");
    let output = lsf(
        &theme.0,
        &[
            "theme",
            "digest",
            "file",
            "snippets/crlf.liquid",
            "./assets/crlf.png",
            "config/../config/settings_data.json",
        ],
    );
    assert_eq!(
        stdout(&output),
        format!(
            "{CRLF_LIQUID}  snippets/crlf.liquid\n{RAW_CRLF}  assets/crlf.png\n{SETTINGS}  config/settings_data.json\n"
        )
    );
    // From elsewhere, with `--theme`: the path is the file's, its name the theme's.
    let parent = theme.0.parent().unwrap();
    let name = theme.0.file_name().unwrap().to_str().unwrap();
    let output = lsf(
        parent,
        &[
            "--theme",
            name,
            "theme",
            "digest",
            "file",
            &format!("{name}/snippets/crlf.liquid"),
        ],
    );
    assert_eq!(
        stdout(&output),
        format!("{CRLF_LIQUID}  snippets/crlf.liquid\n")
    );
}

#[test]
fn a_file_outside_the_theme_or_missing_is_an_error() {
    let theme = Theme::new("errors");
    let outside = std::env::temp_dir().join(format!("lsf-digest-outside-{}", std::process::id()));
    std::fs::write(&outside, b"x").unwrap();
    for (path, message) in [
        (outside.to_str().unwrap(), "is not in the theme"),
        (
            "snippets/none.liquid",
            "snippets/none.liquid does not exist",
        ),
        ("snippets", "snippets is not a file"),
    ] {
        let output = lsf(
            &theme.0,
            &["theme", "digest", "file", "snippets/crlf.liquid", path],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{path}: {stderr}");
        assert!(stderr.starts_with("error: "), "{stderr}");
        assert!(stderr.contains(message), "{path}: {stderr}");
        assert!(output.stdout.is_empty(), "nothing printed before the error");
    }
    let _ = std::fs::remove_file(outside);
    let output = lsf(&theme.0, &["--theme", "none", "theme", "digest"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not exist"));
}

/// The lines `theme digest --list` prints for the test's theme, without the digest.
fn theme_lines(theme: &Theme, globs: &[&str]) -> (Vec<String>, String) {
    let mut args = vec!["theme", "digest", "--list"];
    args.extend(globs);
    let text = stdout(&lsf(&theme.0, &args));
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let summary = lines.pop().unwrap();
    (lines, summary)
}

fn paths(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .map(|line| line.split_once("  ").unwrap().1)
        .collect()
}

#[test]
fn a_theme_has_one_digest_of_its_files_sorted_by_path() {
    let theme = Theme::new("theme");
    let (lines, summary) = theme_lines(&theme, &[]);
    // `assets/basic.css` is generated from `assets/basic.css.liquid`; `README.md` and
    // `.DS_Store` are not files Shopify CLI pushes.
    assert_eq!(
        paths(&lines),
        [
            "assets/basic.css.liquid",
            "assets/crlf.png",
            "config/settings_data.json",
            "layout/theme.liquid",
            "sections/header-group.json",
            "snippets/crlf.liquid",
            "templates/customers/account.json",
            "templates/index.json",
        ]
    );
    assert_eq!(lines[0], format!("{CRLF_LIQUID}  assets/basic.css.liquid"));
    assert_eq!(lines[1], format!("{RAW_CRLF}  assets/crlf.png"));
    assert_eq!(lines[2], format!("{SETTINGS}  config/settings_data.json"));

    // The digest is the SHA-256 of the lines, the same on every run and without `--list`.
    let hashed: String = lines.iter().map(|line| format!("{line}\n")).collect();
    let hex: String = Sha256::digest(hashed.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(summary, format!("sha256:{hex}  8 files"));
    let alone = stdout(&lsf(&theme.0, &["theme", "digest"]));
    assert_eq!(alone, format!("{summary}\n"));
    assert_eq!(stdout(&lsf(&theme.0, &["theme", "digest"])), alone);

    // A change of line ends in a text file is no change; in a binary file, it is one.
    theme.write("snippets/crlf.liquid", b"a\nb\n");
    assert_eq!(stdout(&lsf(&theme.0, &["theme", "digest"])), alone);
    theme.write("assets/crlf.png", b"a\nb\n");
    assert_ne!(stdout(&lsf(&theme.0, &["theme", "digest"])), alone);
}

#[test]
fn includes_keep_files_and_ignores_then_leave_them_out() {
    let theme = Theme::new("globs");
    let (lines, _) = theme_lines(
        &theme,
        &[
            "--ignore",
            "templates/**/*.json",
            "--ignore",
            "sections/*.json",
            "--ignore",
            "config/settings_data.json",
        ],
    );
    assert_eq!(
        paths(&lines),
        [
            "assets/basic.css.liquid",
            "assets/crlf.png",
            "layout/theme.liquid",
            "snippets/crlf.liquid",
        ]
    );
    let (lines, summary) = theme_lines(
        &theme,
        &[
            "--include",
            "templates/**",
            "--include",
            "config/*",
            "--ignore",
            "templates/customers/*",
        ],
    );
    assert_eq!(
        paths(&lines),
        ["config/settings_data.json", "templates/index.json"]
    );
    assert!(summary.ends_with("  2 files"), "{summary}");
    // Globs that match nothing: no file, no error.
    let (lines, summary) = theme_lines(&theme, &["--include", "nothing/**"]);
    assert!(lines.is_empty());
    assert_eq!(
        summary,
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  0 files"
    );
}

#[test]
fn the_options_of_the_digest_are_refused_with_file_and_lsf_digest_is_gone() {
    let theme = Theme::new("spelling");
    let output = lsf(
        &theme.0,
        &["theme", "digest", "--list", "file", "snippets/crlf.liquid"],
    );
    assert_eq!(output.status.code(), Some(2));
    let output = lsf(
        &theme.0,
        &["theme", "digest", "file", "--list", "snippets/crlf.liquid"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(lsf(&theme.0, &["digest", "theme"]).status.code(), Some(2));
    assert_eq!(lsf(&theme.0, &["theme"]).status.code(), Some(2));
    // `--theme` is the global option, before or after the command.
    let parent = theme.0.parent().unwrap();
    let name = theme.0.file_name().unwrap().to_str().unwrap();
    let before = stdout(&lsf(parent, &["--theme", name, "theme", "digest"]));
    let after = stdout(&lsf(parent, &["theme", "digest", "--theme", name]));
    assert_eq!(before, after);
    assert_eq!(before, stdout(&lsf(&theme.0, &["theme", "digest"])));
}
