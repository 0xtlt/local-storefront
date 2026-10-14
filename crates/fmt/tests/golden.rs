//! Differential tests: every case in `tests/golden/*.json` was formatted by Prettier with
//! Shopify's Liquid plugin (see `tools/prettier/generate_golden.mjs`) and must be formatted
//! identically here.
//!
//! The cases are written in `tests/cases/*.liquid`. After adding one, regenerate the
//! expectations with `mise run prettier:golden`.

use std::path::Path;

use lsf_fmt::{Options, format};

/// One case: the file it is in, its name, the options, the source, what Prettier makes of
/// the source, and what Prettier makes of that.
type Case = (String, String, Options, String, String, String);

fn cases() -> Vec<Case> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("tests/golden")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    files.sort();
    let mut cases = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("a golden file");
        let golden: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        for case in golden.as_array().expect("an array of cases") {
            let mut options = Options::default();
            for (name, value) in case["options"].as_object().expect("options") {
                assert!(
                    options.set(name, value).expect("a valid option"),
                    "unknown option {name}"
                );
            }
            cases.push((
                file.file_stem().unwrap().to_string_lossy().into_owned(),
                case["name"].as_str().expect("name").to_string(),
                options,
                case["source"].as_str().expect("source").to_string(),
                case["expected"].as_str().expect("expected").to_string(),
                case["again"].as_str().expect("again").to_string(),
            ));
        }
    }
    cases
}

#[test]
fn formats_as_prettier_does() {
    let cases = cases();
    assert!(cases.len() >= 60, "golden files are missing");
    let mut failures = Vec::new();
    for (file, name, options, source, expected, _) in &cases {
        let actual = format(source, options);
        if actual != *expected {
            failures.push(format!(
                "{file}: {name}\n--- source\n{source}--- Prettier\n{expected}--- lsf fmt\n{actual}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ from Prettier:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// Formatting what is formatted gives what it gives Prettier: the same, but for a source
/// that cannot be parsed.
#[test]
fn formats_its_own_output_as_prettier_does() {
    for (file, name, options, _, expected, again) in &cases() {
        assert_eq!(
            format(expected, options),
            *again,
            "{file}: {name}: the formatted source, formatted again"
        );
    }
}

/// A source cut anywhere is still formatted: nothing is refused, and nothing panics.
#[test]
fn formats_a_source_cut_anywhere() {
    for (_, _, options, source, _, _) in &cases() {
        for (end, _) in source.char_indices() {
            format(&source[..end], options);
            // The same, starting in the middle of something.
            format(&source[end..], options);
        }
    }
}
