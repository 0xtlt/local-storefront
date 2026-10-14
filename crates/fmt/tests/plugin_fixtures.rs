//! Conformance against the tests of Shopify's Prettier plugin for Liquid
//! (<https://github.com/Shopify/theme-tools>, `packages/prettier-plugin-liquid/src/test`).
//!
//! `mise run prettier:fixtures` downloads them into `.cache/theme-tools` and has Prettier
//! format each case, with the options the case asks for, into
//! `.cache/prettier-plugin-liquid-cases.json` (see `tools/prettier/export_fixtures.mjs`). This
//! test formats every case with `lsf-fmt` and compares.
//!
//! The cases that differ are those whose JavaScript Prettier reformats: `lsf-fmt` leaves
//! JavaScript as it is written. The number to watch is the pass count: it must not go down.
//!
//! Run with `cargo test -p lsf-fmt --test plugin_fixtures -- --nocapture` to see the report,
//! and with `PLUGIN_FIXTURES_VERBOSE=1` to see every difference. The test is skipped when the
//! cases have not been downloaded.

use std::path::Path;

use lsf_fmt::{Options, format};

/// The revision of the plugin's tests the baseline was recorded against.
const REVISION: &str = include_str!("../../../tools/prettier/FIXTURES_REVISION");

/// How many cases must be formatted as Prettier formats them, of 444. Raise the number when
/// more are; never lower it.
const BASELINE: usize = 443;

#[test]
fn formats_the_tests_of_the_plugin_as_prettier_does() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Ok(text) = std::fs::read_to_string(root.join(".cache/prettier-plugin-liquid-cases.json"))
    else {
        eprintln!("skipped: run `mise run prettier:fixtures` to download the cases");
        return;
    };
    let downloaded = std::process::Command::new("git")
        .args(["-C", ".cache/theme-tools", "rev-parse", "HEAD"])
        .current_dir(&root)
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    if downloaded.as_deref() != Some(REVISION.trim()) {
        eprintln!("skipped: the downloaded cases are not those of FIXTURES_REVISION");
        return;
    }

    let cases: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let cases = cases.as_array().expect("an array of cases");
    let verbose = std::env::var_os("PLUGIN_FIXTURES_VERBOSE").is_some();
    let mut passed = 0;
    let mut different = Vec::new();
    for case in cases {
        let mut options = Options::default();
        for (name, value) in case["options"].as_object().expect("options") {
            // Options that apply to JavaScript, which is not reformatted, are not known.
            options.set(name, value).expect("a valid option");
        }
        let source = case["source"].as_str().expect("source");
        let expected = case["expected"].as_str().expect("expected");
        let actual = format(source, &options);
        if actual == expected {
            passed += 1;
        } else {
            let name = case["name"].as_str().expect("name");
            different.push(name);
            if verbose {
                eprintln!(
                    "{name}\n--- source\n{source}\n--- Prettier\n{expected}--- lsf fmt\n{actual}"
                );
            }
        }
    }
    eprintln!(
        "{passed} of {} cases formatted as Prettier formats them; different: {}",
        cases.len(),
        different.join(", ")
    );
    assert!(
        passed >= BASELINE,
        "{passed} cases pass, {BASELINE} did: {}",
        different.join(", ")
    );
}
