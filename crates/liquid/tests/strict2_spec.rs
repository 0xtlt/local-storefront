//! Shopify's strict parser (`lsf_liquid::strict2`) against liquid-spec.
//!
//! liquid-spec runs most of its specs in the gem's `strict2` mode: the ones that expect a parse
//! error say what the strict parser rejects, and the others are templates it accepts. For each
//! spec this test audits the template and checks that a rejection is found exactly when the
//! spec expects one, with the gem's message. The fixes are checked too: a fixed template must
//! audit clean.
//!
//! The suite is downloaded with `mise run liquid-spec:fetch`; the test is skipped without it.
//! `STRICT2_SPEC_VERBOSE=1` prints every difference. `STRICT2_SPEC_FIXES=<file>` writes every
//! template that has a fix, with the fixed template and the spec's data, as JSON for
//! `mise run oracle:strict2-fixes <file>`, which checks with the reference gem that each fixed
//! template renders in `strict2` mode what the original renders today.

use std::path::{Path, PathBuf};

use lsf_liquid::Environment;
use lsf_liquid::strict2::{self, IssueKind};
use serde_json::Value as Json;

/// The number of specs that must agree; raise it when more do, never lower it.
const BASELINE: usize = 6314;

/// Specs that expect a template to be accepted that the reference gem rejects in `strict2` mode:
/// recordings of Shopify's production, which still parses leniently.
const GEM_REJECTS: &[&str] = &[
    "RecordedTest#test_string_first_65823d54f",
    "RecordedTest#test_string_last_035516211",
];

/// Features whose templates need what this crate does not have.
const SKIPPED_FEATURES: &[&str] = &[
    "ruby_types",
    "ruby_drops",
    "binary_data",
    "template_factory",
    "shopify_tags",
    "shopify_objects",
    "shopify_filters",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Whether liquid-spec runs the spec in `strict2` mode: specs without modes run in the
/// strictest mode an adapter supports.
fn runs_in_strict2(spec: &Json) -> bool {
    match spec["error_modes"].as_array() {
        Some(modes) if !modes.is_empty() => modes.iter().any(|mode| mode == "strict2"),
        _ => true,
    }
}

fn skipped(spec: &Json) -> bool {
    let features = spec["features"].as_array().cloned().unwrap_or_default();
    features
        .iter()
        .any(|feature| SKIPPED_FEATURES.contains(&feature.as_str().unwrap_or_default()))
        || !spec["template"].is_string()
        || !spec["template_factory"].is_null()
}

fn patterns(spec: &Json) -> Vec<String> {
    match &spec["errors"]["parse_error"] {
        Json::Array(items) => items
            .iter()
            .map(|item| item.as_str().unwrap_or_default().to_lowercase())
            .collect(),
        Json::String(item) => vec![item.to_lowercase()],
        _ => Vec::new(),
    }
}

#[test]
fn agrees_with_liquid_spec() {
    let directory = workspace_root().join(".cache/liquid-spec-cases");
    if !directory.exists() {
        eprintln!("liquid-spec is not downloaded: run `mise run liquid-spec:fetch`");
        return;
    }
    let verbose = std::env::var("STRICT2_SPEC_VERBOSE").is_ok();
    let mut fixes = Vec::new();
    let env = Environment::standard();
    let mut agree = 0;
    let mut differences = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&directory)
        .expect("liquid-spec cases")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter(|path| !path.ends_with("shopify_theme_dawn.json"))
        .collect();
    files.sort();
    for path in files {
        let suite: Json = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for spec in suite["specs"].as_array().into_iter().flatten() {
            if !runs_in_strict2(spec)
                || skipped(spec)
                || GEM_REJECTS.contains(&spec["name"].as_str().unwrap_or_default())
            {
                continue;
            }
            let template = spec["template"].as_str().unwrap();
            let expected = patterns(spec);
            let audit = strict2::audit(&env, template);
            let rejection = audit
                .findings
                .iter()
                .find(|finding| finding.issue.kind == IssueKind::Rejected);
            let message = match (rejection, &audit.error) {
                (Some(finding), _) => Some(format!(
                    "SyntaxError: {} in \"{}\"",
                    finding.issue.message,
                    finding.markup(template)
                )),
                (None, Some(error)) => Some(format!("SyntaxError: {error}")),
                (None, None) => None,
            };
            let problem = match (&message, expected.is_empty()) {
                (None, true) => None,
                (Some(message), true) => Some(format!("rejected: {message}")),
                (None, false) => Some(format!("accepted, expected {expected:?}")),
                (Some(message), false) => {
                    let lower = message.to_lowercase();
                    (!expected
                        .iter()
                        .all(|pattern| lower.contains(pattern.as_str())))
                    .then(|| format!("{message}, expected {expected:?}"))
                }
            };
            // A fix must leave nothing for the strict parser to reject.
            let fixed = strict2::apply(template, &audit.findings);
            // What `lsf check --fix` writes, when it differs from the fixes that keep the output.
            let meant = strict2::apply_intents(&env, template, |_, _| true).ok();
            if fixed != template {
                fixes.push(serde_json::json!({
                    "name": spec["name"],
                    "template": template,
                    "fixed": fixed,
                    "meant": meant.filter(|meant| *meant != fixed),
                    // A fix that changes the output on purpose only has to parse strictly.
                    "changes": audit.findings.iter().any(|finding| finding.issue.changes.is_some()),
                    "environment": spec["environment"],
                    "filesystem": spec["filesystem"],
                }));
            }
            let problem = problem.or_else(|| {
                let after = strict2::audit(&env, &fixed);
                (fixed != template
                    && after
                        .findings
                        .iter()
                        .any(|finding| finding.issue.fix.is_ok()))
                .then(|| format!("the fix {fixed:?} still has findings"))
            });
            match problem {
                None => agree += 1,
                Some(problem) => differences.push(format!(
                    "{} ({}): {template:?}: {problem}",
                    spec["name"].as_str().unwrap_or_default(),
                    path.file_name().unwrap().to_string_lossy()
                )),
            }
        }
    }
    if verbose {
        for difference in &differences {
            eprintln!("{difference}");
        }
    }
    eprintln!(
        "strict2: {agree} specs agree, {} differ, {} have a fix",
        differences.len(),
        fixes.len()
    );
    if let Ok(path) = std::env::var("STRICT2_SPEC_FIXES") {
        std::fs::write(&path, serde_json::to_string_pretty(&fixes).unwrap()).unwrap();
    }
    assert!(
        agree >= BASELINE,
        "{agree} specs agree with liquid-spec, below the baseline of {BASELINE}"
    );
}
