//! Differential tests: every case in `tests/golden/*.json` was rendered by Shopify's reference
//! Liquid gem (see `tools/oracle/generate_golden.rb`) and must render identically here.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chrono_tz::Tz;
use lsf_liquid::{Context, Environment, Error, PartialLoader, Template, Value};

struct Partials {
    env: Arc<Environment>,
    sources: HashMap<String, String>,
}

impl PartialLoader for Partials {
    fn load(&self, name: &str) -> Result<Arc<Template>, Error> {
        let source = self
            .sources
            .get(name)
            .ok_or_else(|| Error::file_system(format!("No such template '{name}'")))?;
        Template::parse_named(&self.env, source, Some(name)).map(Arc::new)
    }
}

fn render(env: &Arc<Environment>, case: &serde_json::Value) -> String {
    let source = case["template"].as_str().expect("template");
    let template = match Template::parse(env, source) {
        Ok(template) => template,
        Err(error) => return format!("SYNTAX: {error}"),
    };
    let sources = case["partials"]
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(name, source)| {
                    (
                        name.clone(),
                        source.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut builder = Context::builder(env.clone())
        .time_zone(Tz::UTC)
        .partials(Arc::new(Partials {
            env: env.clone(),
            sources,
        }));
    if let Some(data) = case["data"].as_object() {
        for (key, value) in data {
            builder = builder.assign(key.clone(), Value::from(value));
        }
    }
    let mut ctx = builder.build();
    template.render(&mut ctx)
}

/// The environment a case is rendered with: the standard one, or the one with the parse
/// options the case was rendered with by the reference implementation.
fn environment(case: &serde_json::Value) -> Environment {
    let mut env = Environment::standard();
    if let Some(options) = case["parse"].as_object() {
        for (name, value) in options {
            match name.as_str() {
                "bug_compatible_whitespace_trimming" => {
                    env.set_bug_compatible_whitespace_trimming(value.as_bool().unwrap_or(false));
                }
                other => panic!("unknown parse option {other}"),
            }
        }
    }
    env
}

#[test]
fn matches_the_reference_implementation() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("golden directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no golden files in {}", dir.display());

    let only = std::env::var("GOLDEN_ONLY").ok();
    let mut total = 0;
    let mut failures = Vec::new();
    for file in files {
        let name = file.file_stem().unwrap().to_string_lossy().into_owned();
        if only
            .as_ref()
            .is_some_and(|only| !name.contains(only.as_str()))
        {
            continue;
        }
        let golden: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        for case in golden["cases"].as_array().unwrap() {
            total += 1;
            let expected = case["expected"].as_str().unwrap();
            let actual = render(&Arc::new(environment(case)), case);
            if actual != expected {
                failures.push(format!(
                    "{name}.txt:{}\n  template: {:?}\n  expected: {:?}\n  actual:   {:?}",
                    case["line"], case["template"], expected, actual
                ));
            }
        }
    }
    if !failures.is_empty() {
        let shown = failures
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "{} of {total} golden cases differ:\n{shown}",
            failures.len()
        );
    }
}
