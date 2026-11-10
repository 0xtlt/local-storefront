//! `lsf validate`: the store data of a theme, and of an app without a theme (its checkout
//! scenarios), each in a folder of the test's own.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A folder of the test's, removed with the value.
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Folder {
        let path = std::env::temp_dir().join(format!("lsf-validate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Folder(path)
    }

    fn write(&self, file: &str, text: &str) {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn validate(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lsf"))
        .arg("validate")
        .args(args)
        .current_dir(directory)
        .env_remove("LSF_THEME")
        .env_remove("LSF_DATA")
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

const SHOP: &str = r#"{"shop": {"name": "Test shop", "currency": "EUR"}}"#;

#[test]
fn an_app_without_a_theme_has_its_store_data_and_scenarios_validated() {
    let app = Folder::new("app");
    app.write("shopify-local/shop.json", SHOP);
    app.write(
        "shopify-local/checkouts/fine.json",
        r#"{"session": {"cart": {"items": []}}}"#,
    );
    let output = validate(&app.0, &[]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("valid (0 products, ")
            && text(&output).contains("1 checkout scenario)"),
        "{}",
        text(&output)
    );

    // A scenario that does not validate is an error, as with a theme.
    app.write(
        "shopify-local/checkouts/broken.json",
        r#"{"session": {"cart": {"items": [{"variant": "NOPE"}]}}}"#,
    );
    let output = validate(&app.0, &["--format", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["ok"], false);
    assert_eq!(report["diagnostics"][0]["file"], "checkouts/broken.json");
}

#[test]
fn a_folder_with_neither_a_theme_nor_store_data_says_so() {
    let empty = Folder::new("empty");
    let output = validate(&empty.0, &[]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("has no store data to validate"),
        "{}",
        text(&output)
    );
}

#[test]
fn a_theme_is_still_opened_as_one() {
    let theme = Folder::new("theme");
    theme.write("layout/theme.liquid", "{{ content_for_layout }}");
    theme.write("shopify-local/shop.json", SHOP);
    let output = validate(&theme.0, &[]);
    assert!(output.status.success(), "{}", text(&output));
    // Without data, a theme validates the demo store, as before.
    std::fs::remove_dir_all(theme.0.join("shopify-local")).unwrap();
    let output = validate(&theme.0, &[]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("built-in demo store"),
        "{}",
        text(&output)
    );
}

/// A command that only prints, piped into a reader that stops early (`lsf docs | head`), ends
/// as `cat` does, by the signal of the closed pipe, without a panic.
#[cfg(unix)]
#[test]
fn a_reader_that_stops_early_ends_a_printing_command_quietly() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_lsf"))
        .arg("docs")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The guide is longer than a pipe holds: lsf is still writing when the reader leaves.
    let mut first = [0u8; 16];
    child.stdout.take().unwrap().read_exact(&mut first).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(&first, b"# Store data for");
    // SIGPIPE.
    assert_eq!(output.status.signal(), Some(13), "{:?}", output.status);
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}
