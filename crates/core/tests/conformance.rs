//! Conformance against Shopify's own documentation.
//!
//! Every page of Shopify's Liquid reference shows examples as a triple: the Liquid code, the
//! data it ran against and the output Shopify's renderer produced. `mise run docs:fetch`
//! downloads those pages into `.cache/shopify-docs`; this test replays each example and
//! compares our output with Shopify's.
//!
//! Whitespace is ignored (see `normalize`). The examples run against Shopify's demo store, which we only know through the data shown
//! next to each example. Examples that depend on more than that data cannot pass and are
//! listed in the report as such, so the number to watch is the pass count: it must not go down.
//!
//! Run with `cargo test -p lsf-core --test conformance -- --nocapture` to see the report. The
//! test is skipped when the pages have not been downloaded.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsf_core::render::page::{Page, Resource};
use lsf_core::store::load::{DataSource, LoadOptions, load_with_overlay};
use lsf_core::theme::Revalidate;
use lsf_core::{Renderer, Request, Session, Theme};
use lsf_liquid::Value;

/// The number of examples that must pass. Raise it when more pass; never lower it.
const BASELINE: usize = 195;

struct Example {
    page: String,
    code: String,
    data: Option<serde_json::Value>,
    output: String,
}

/// Extracts the `Code` / `Data` / `Output` blocks of a reference page.
fn examples(page: &str, markdown: &str) -> Vec<Example> {
    let mut out = Vec::new();
    let mut section: Option<&str> = None;
    let mut buffer: Option<String> = None;
    let mut code: Option<String> = None;
    let mut data: Option<serde_json::Value> = None;
    for line in markdown.lines() {
        if let Some(title) = line.strip_prefix("##### ") {
            section = match title.trim() {
                "Code" => {
                    code = None;
                    data = None;
                    Some("code")
                }
                "Data" => Some("data"),
                "Output" => Some("output"),
                _ => None,
            };
            continue;
        }
        if line.starts_with("```") {
            match buffer.take() {
                Some(mut text) => {
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    match section {
                        Some("code") => code = Some(text),
                        Some("data") => data = serde_json::from_str(&text).ok(),
                        Some("output") => {
                            if let Some(code) = code.take() {
                                out.push(Example {
                                    page: page.to_string(),
                                    code,
                                    data: data.take(),
                                    output: text,
                                });
                            }
                        }
                        _ => {}
                    }
                    section = None;
                }
                None if section.is_some() => buffer = Some(String::new()),
                None => {}
            }
            continue;
        }
        if let Some(text) = &mut buffer {
            text.push_str(line);
            text.push('\n');
        }
    }
    out
}

/// Removes what legitimately differs between Shopify's CDN and the local server: asset
/// versions, the theme id, and the insignificant whitespace the documentation trims.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("v=") {
        let before = &rest[..index];
        let after = &rest[index + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let boundary = before.ends_with(['?', '&', ';']);
        out.push_str(before);
        if boundary && digits > 0 {
            out.push_str("v=N");
            rest = &after[digits..];
        } else {
            out.push_str("v=");
            rest = after;
        }
    }
    out.push_str(rest);
    let out = out.replace("/cdn/shop/t/4/", "/cdn/shop/t/1/");
    // The documentation re-flows the output of its examples (it shows one item per line where
    // Liquid's whitespace control actually removes the line breaks), so whitespace cannot be
    // compared. Exact whitespace behaviour is covered by the tests against the reference
    // implementation in `lsf-liquid`.
    out.chars().filter(|c| !c.is_whitespace()).collect()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn matches_shopify_documentation_examples() {
    let docs = workspace_root().join(".cache/shopify-docs");
    if !docs.is_dir() {
        eprintln!("skipped: run `mise run docs:fetch` to download Shopify's reference pages");
        return;
    }

    // A minimal theme: the examples only need the Liquid environment.
    let theme_dir = std::env::temp_dir().join(format!("lsf-conformance-{}", std::process::id()));
    for directory in [
        "layout",
        "templates",
        "locales",
        "assets",
        "snippets",
        "sections",
        "config",
    ] {
        std::fs::create_dir_all(theme_dir.join(directory)).unwrap();
    }
    std::fs::write(
        theme_dir.join("layout/theme.liquid"),
        "{{ content_for_layout }}",
    )
    .unwrap();
    std::fs::write(theme_dir.join("locales/en.default.json"), "{}").unwrap();
    std::fs::write(
        theme_dir.join("assets/icon.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .unwrap();

    let env = Arc::new(lsf_core::environment());
    let theme = Arc::new(Theme::open(&theme_dir, env, Revalidate::Never).unwrap());
    let renderer = Renderer::new(theme);
    // The shop the documentation's examples were rendered on.
    let overlay = serde_json::json!({
        "shop": {
            "name": "Polina's Potent Potions",
            "currency": "CAD",
            "money_format": "${{amount}}",
            "money_with_currency_format": "${{amount}} CAD",
            "timezone": "America/Toronto"
        },
        "now": "2023-05-10T12:00:00Z"
    });
    let (store, diagnostics) =
        load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
    assert!(!diagnostics.has_errors(), "{diagnostics}");
    let store = Arc::new(store);

    let mut all = Vec::new();
    for kind in ["filters", "tags", "objects"] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(docs.join(kind))
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        files.sort();
        for file in files {
            let name = format!("{kind}/{}", file.file_stem().unwrap().to_string_lossy());
            all.extend(examples(&name, &std::fs::read_to_string(&file).unwrap()));
        }
    }

    let only = std::env::var("CONFORMANCE_ONLY").ok();
    let verbose = std::env::var("CONFORMANCE_VERBOSE").is_ok();
    let mut passed = 0;
    let mut failed: Vec<(String, String, String, String)> = Vec::new();
    for example in &all {
        if only
            .as_ref()
            .is_some_and(|only| !example.page.contains(only.as_str()))
        {
            continue;
        }
        let request = Request::new(
            "polinas-potent-potions.myshopify.com",
            "/services/liquid_rendering/resource",
        );
        let session = Session::initial(&store);
        let site = renderer.site(store.clone(), request, session);
        let variables: Vec<(String, Value)> = example
            .data
            .as_ref()
            .and_then(|data| data.as_object())
            .map(|data| {
                data.iter()
                    .map(|(key, value)| (key.clone(), Value::from(value)))
                    .collect()
            })
            .unwrap_or_default();
        let actual = match renderer.render_liquid(
            &site,
            Page::new("index", Resource::Index),
            &example.code,
            &variables,
        ) {
            Ok((output, _)) => output,
            Err(error) => error.to_string(),
        };
        if normalize(&actual) == normalize(&example.output) {
            passed += 1;
        } else {
            failed.push((
                example.page.clone(),
                example.code.clone(),
                example.output.clone(),
                actual,
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&theme_dir);

    let total = passed + failed.len();
    eprintln!("conformance: {passed}/{total} documentation examples match Shopify's output");
    let mut by_page: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for (page, ..) in &failed {
        *by_page.entry(page.as_str()).or_default() += 1;
    }
    eprintln!(
        "differing: {}",
        by_page
            .iter()
            .map(|(page, count)| format!("{page}({count})"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    if verbose {
        for (page, code, expected, actual) in &failed {
            let clip = |text: &str| text.chars().take(600).collect::<String>();
            eprintln!(
                "\n=== {page}\n--- code\n{}\n--- shopify\n{}\n--- local\n{}",
                clip(code),
                clip(&normalize(expected)),
                clip(&normalize(actual))
            );
        }
    }
    if only.is_none() {
        assert!(
            passed >= BASELINE,
            "conformance regressed: {passed} examples pass, the baseline is {BASELINE}"
        );
    }
}
