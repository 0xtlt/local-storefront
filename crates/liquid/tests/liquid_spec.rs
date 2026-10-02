//! Conformance against Shopify's liquid-spec, the test suite of the Liquid language
//! (<https://github.com/Shopify/liquid-spec>).
//!
//! `mise run liquid-spec:fetch` downloads the suite into `.cache/liquid-spec` and converts its
//! specs to JSON in `.cache/liquid-spec-cases` (see `tools/liquid-spec/export.rb`). This test
//! plays the part of a liquid-spec adapter: it parses and renders every spec with `lsf-liquid`
//! and compares the result the way liquid-spec's own runner does.
//!
//! Not every spec applies. The engine parses templates like Shopify does for themes (strictly,
//! falling back to the forgiving legacy parser), so the specs that expect a strict parse mode
//! to reject a template are skipped, as are the ones that need Ruby objects;
//! `SKIPPED_FEATURES` lists the reasons. What is left either passes or is a difference with the
//! reference implementation, so the numbers to watch are the pass counts: they must not go
//! down.
//!
//! The suite `shopify_theme_dawn` is not run: its specs are sections of the Dawn theme without
//! the snippets, translations and settings they were rendered with.
//!
//! Run with `cargo test -p lsf-liquid --test liquid_spec -- --nocapture` to see the report.
//! `LIQUID_SPEC_ONLY=<text>` keeps the specs whose file or name contains the text,
//! `LIQUID_SPEC_VERBOSE=1` prints every difference and `LIQUID_SPEC_REPORT=<file>` writes them
//! as JSON. The test is skipped when the suite has not been downloaded.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use lsf_liquid::{
    Context, Environment, Error, ErrorKind, Hash, Object, PartialLoader, Template, Value,
};
use serde_json::Value as Json;

/// The revision of liquid-spec the baselines were recorded against.
const REVISION: &str = include_str!("../../../tools/liquid-spec/REVISION");

/// The suites that test the language, and the number of their specs that must pass.
/// Raise a number when more pass; never lower it.
const BASELINE: &[(&str, usize)] = &[
    ("basics", 923),
    ("liquid_ruby", 1658),
    ("liquid_ruby_lax", 118),
    ("parser_errors", 0),
    ("partials", 12),
    ("shopify_production_recordings", 1887),
    ("benchmarks", 10),
];

/// Feature tags of specs that are not run, with the reason.
const SKIPPED_FEATURES: &[(&str, &str)] = &[
    (
        "ruby_types",
        "Ruby values (symbols, hashes with non-string keys)",
    ),
    ("ruby_drops", "drop classes of the Ruby test suite"),
    ("drop_class_output", "prints a Ruby class name"),
    ("binary_data", "byte strings that are not UTF-8"),
    ("activesupport", "ActiveSupport's SafeBuffer"),
    ("template_factory", "Ruby template factory callbacks"),
    (
        "strict2_blank_body_errors",
        "a contract the reference does not implement yet",
    ),
    ("shopify_resource_limits", "render score limits"),
    ("range_resource_limits", "render score limits"),
    ("shopify_tags", "Shopify tags live in lsf-core"),
    ("shopify_objects", "Shopify objects live in lsf-core"),
    ("shopify_filters", "Shopify filters live in lsf-core"),
];

/// A spec that renders for longer than this is reported as a difference.
const TIMEOUT: Duration = Duration::from_secs(20);

enum Outcome {
    Pass,
    Fail { expected: String, actual: String },
    Skip(String),
}

// --- environment ---------------------------------------------------------------------------

/// The portable test drops of liquid-spec (`docs/test_drops.md`).
enum Drop {
    /// `BooleanDrop`, `NumberDrop`, `StringDrop`: stand for the value they wrap.
    Boxed(Value),
    /// `drop.echo_N`, `drop.square_N`, `drop.double_N`.
    Method,
    /// `drop[0]` is `zero`, `drop["foo"]` is `foo`.
    Index,
    /// Iterates over `first`, `second`, `third`.
    Sequence,
    /// Truthy, prints `opaque`.
    Opaque,
    /// `drop.nested` is the drop itself, `drop.square` a drop holding the squared value.
    Nested(i64),
}

impl Object for Drop {
    fn type_name(&self) -> &str {
        "test"
    }

    fn get(&self, key: &str) -> Option<Value> {
        match self {
            Drop::Method => {
                let (operation, number) = key.split_once('_')?;
                if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let n: i64 = number.parse().ok()?;
                let result = match operation {
                    "echo" => n,
                    "square" => n * n,
                    "double" => n * 2,
                    _ => return None,
                };
                Some(Value::str(result.to_string()))
            }
            Drop::Index => Some(Value::str(key)),
            Drop::Nested(value) => match key {
                "nested" => Some(Value::object(Drop::Nested(*value))),
                "square" => Some(Value::object(Drop::Nested(value * value))),
                _ => None,
            },
            _ => None,
        }
    }

    fn index(&self, index: i64) -> Option<Value> {
        match self {
            Drop::Index => {
                const WORDS: [&str; 6] = ["zero", "one", "two", "three", "four", "five"];
                let word = usize::try_from(index).ok().and_then(|i| WORDS.get(i));
                Some(Value::str(word.unwrap_or(&"unknown")))
            }
            _ => None,
        }
    }

    fn items(&self) -> Option<Arc<Vec<Value>>> {
        match self {
            Drop::Sequence => Some(Arc::new(
                ["first", "second", "third"].map(Value::str).to_vec(),
            )),
            _ => None,
        }
    }

    fn to_value(&self) -> Option<Value> {
        match self {
            Drop::Boxed(value) => Some(value.clone()),
            Drop::Nested(value) => Some(Value::Int(*value)),
            _ => None,
        }
    }

    fn render(&self) -> Cow<'_, str> {
        match self {
            Drop::Boxed(value) => Cow::Owned(value.to_str().into_owned()),
            Drop::Nested(value) => Cow::Owned(value.to_string()),
            Drop::Opaque => Cow::Borrowed("opaque"),
            _ => Cow::Borrowed(""),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Builds the object a spec asks for with `instantiate:Class`. `Err` names what the engine has
/// no equivalent for.
fn instantiate(class: &str, params: &Json, standard_drops: bool) -> Result<Value, String> {
    let value = || params.get("value").cloned().unwrap_or(Json::Null);
    match class {
        "Range" => {
            let bound = |index: usize| params.get(index).and_then(Json::as_i64);
            let (Some(from), Some(to)) = (bound(0), bound(1)) else {
                return Err("a range that is not made of integers".to_string());
            };
            let exclusive = params.get(2).and_then(Json::as_bool).unwrap_or(false);
            Ok(Value::Range(from, if exclusive { to - 1 } else { to }))
        }
        "LongString" => {
            let length = params.get("length").and_then(Json::as_u64).unwrap_or(0);
            Ok(Value::str("X".repeat(length as usize)))
        }
        _ if !standard_drops => Err(format!("the Ruby class {class}")),
        "BooleanDrop" => Ok(Value::object(Drop::Boxed(Value::Bool(
            value().as_bool().unwrap_or(false),
        )))),
        "NumberDrop" => Ok(Value::object(Drop::Boxed(Value::Int(
            value().as_i64().unwrap_or(0),
        )))),
        "StringDrop" => Ok(Value::object(Drop::Boxed(to_value(&value(), true)?))),
        "MethodDrop" => Ok(Value::object(Drop::Method)),
        "IndexDrop" => Ok(Value::object(Drop::Index)),
        "SequenceDrop" => Ok(Value::object(Drop::Sequence)),
        "NilDrop" => Ok(Value::Nil),
        "OpaqueDrop" => Ok(Value::object(Drop::Opaque)),
        "NestedDrop" => Ok(Value::object(Drop::Nested(value().as_i64().unwrap_or(3)))),
        // An `Object` cannot fail a lookup, which is all `ErrorDrop` does.
        _ => Err(format!("the drop {class}")),
    }
}

/// `instantiate:Class`, `instantiate:Class:` or `instantiate:Class.new(argument)`.
fn instantiate_marker(text: &str) -> Option<(&str, Option<&str>)> {
    let rest = text.strip_prefix("instantiate:")?;
    let rest = rest.strip_suffix(':').unwrap_or(rest);
    let word = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
    if let Some((class, argument)) = rest.split_once(".new(")
        && let Some(argument) = argument.strip_suffix(')')
        && word(class)
    {
        return Some((class, Some(argument)));
    }
    word(rest).then_some((rest, None))
}

/// Converts a value of a spec's environment. `Err` names the Ruby type the engine has no
/// equivalent for.
fn to_value(json: &Json, standard_drops: bool) -> Result<Value, String> {
    Ok(match json {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::Int(i),
            None => Value::Float(n.as_f64().unwrap_or(0.0)),
        },
        Json::String(s) => match instantiate_marker(s) {
            Some((class, argument)) => {
                let params = match argument {
                    Some(argument) => match argument.parse::<i64>() {
                        Ok(i) => Json::from(i),
                        Err(_) => Json::from(argument),
                    },
                    None => Json::Object(Default::default()),
                };
                instantiate(class, &params, standard_drops)?
            }
            None => Value::str(s),
        },
        Json::Array(items) => Value::array(
            items
                .iter()
                .map(|item| to_value(item, standard_drops))
                .collect::<Result<_, _>>()?,
        ),
        Json::Object(map) => {
            if let Some(kind) = map.get("$type").and_then(Json::as_str) {
                return match kind {
                    "range" => {
                        let from = map.get("begin").and_then(Json::as_i64);
                        let to = map.get("end").and_then(Json::as_i64);
                        let exclusive = map.get("exclude_end").and_then(Json::as_bool);
                        match (from, to) {
                            (Some(from), Some(to)) => Ok(Value::Range(
                                from,
                                if exclusive == Some(true) { to - 1 } else { to },
                            )),
                            _ => Err("a range that is not made of integers".to_string()),
                        }
                    }
                    "float" => Ok(Value::Float(
                        match map.get("value").and_then(Json::as_str) {
                            Some("Infinity") => f64::INFINITY,
                            Some("-Infinity") => f64::NEG_INFINITY,
                            _ => f64::NAN,
                        },
                    )),
                    "symbol" => Err("a Ruby symbol".to_string()),
                    "hash" => Err("a hash with keys that are not strings".to_string()),
                    "time" | "date" => Err("a Ruby Time or Date".to_string()),
                    "integer" => Err("an integer larger than 64 bits".to_string()),
                    "binary" => Err("a byte string that is not UTF-8".to_string()),
                    "cycle" => Err("a structure that contains itself".to_string()),
                    other => Err(format!("a Ruby {other}")),
                };
            }
            if map.len() == 1
                && let Some((key, params)) = map.iter().next()
                && let Some((class, None)) = instantiate_marker(key)
            {
                return instantiate(class, params, standard_drops);
            }
            let mut hash = Hash::new();
            for (key, value) in map {
                hash.insert(key.clone(), to_value(value, standard_drops)?);
            }
            Value::hash(hash)
        }
    })
}

// --- partials ------------------------------------------------------------------------------

/// The spec's `environment`. liquid-spec hands it to the engine as static environments: read
/// by every partial, and never written to (`increment` counts from zero next to it).
struct StaticEnvironment(Hash);

impl Object for StaticEnvironment {
    fn type_name(&self) -> &str {
        "environment"
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.0.get(key).cloned()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The spec's `filesystem`. A file is found by its exact name, then the way liquid-spec's own
/// file system finds it: without the case, and with or without `.liquid`.
struct FileSystem {
    env: Arc<Environment>,
    files: Vec<(String, String)>,
    missing: Option<String>,
}

fn file_key(name: &str) -> String {
    let name = name.to_lowercase();
    if name.ends_with(".liquid") {
        name
    } else {
        format!("{name}.liquid")
    }
}

impl PartialLoader for FileSystem {
    fn load(&self, name: &str) -> Result<Arc<Template>, Error> {
        let key = file_key(name);
        let source = self
            .files
            .iter()
            .find(|(file, _)| file == name)
            .or_else(|| {
                self.files
                    .iter()
                    .rev()
                    .find(|(file, _)| file_key(file) == key)
            })
            .map(|(_, source)| source)
            .ok_or_else(|| {
                Error::file_system(match &self.missing {
                    Some(message) => message.clone(),
                    None => format!("Could not find asset {name}"),
                })
            })?;
        Template::parse_named(&self.env, source, Some(name)).map(Arc::new)
    }
}

// --- comparison ----------------------------------------------------------------------------

/// The class the reference implementation raises for an error.
fn class_name(error: &Error) -> &'static str {
    match error.kind {
        ErrorKind::Syntax => "Liquid::SyntaxError",
        ErrorKind::Argument => "Liquid::ArgumentError",
        ErrorKind::ZeroDivision => "Liquid::ZeroDivisionError",
        ErrorKind::FileSystem => "Liquid::FileSystemError",
        ErrorKind::StackLevel => "Liquid::StackLevelError",
        ErrorKind::Disabled => "Liquid::DisabledError",
        ErrorKind::Standard => "Liquid::Error",
        ErrorKind::Internal => "Liquid::InternalError",
    }
}

/// What follows the location in `Liquid error (line 1): message`.
fn core_message(text: &str) -> &str {
    let after = |separator: &str| {
        text.find(separator)
            .map(|index| text[index + separator.len()..].trim())
            .filter(|rest| !rest.is_empty())
    };
    after("):").or_else(|| after(":")).unwrap_or(text)
}

fn patterns(spec: &Json, kind: &str) -> Vec<String> {
    match &spec["errors"][kind] {
        Json::Array(items) => items.iter().map(pattern_text).collect(),
        Json::Null => Vec::new(),
        other => vec![pattern_text(other)],
    }
}

fn pattern_text(pattern: &Json) -> String {
    match pattern {
        Json::String(s) => s.to_lowercase(),
        other => other.to_string().to_lowercase(),
    }
}

/// Every pattern is a substring, without case, of one of the texts.
fn all_match(patterns: &[String], texts: &[&str]) -> bool {
    let texts: Vec<String> = texts.iter().map(|text| text.to_lowercase()).collect();
    patterns
        .iter()
        .all(|pattern| texts.iter().any(|text| text.contains(pattern.as_str())))
}

fn mentions_line(text: &str, line: i64) -> bool {
    let needle = format!("line {line}");
    let text = text.to_lowercase();
    text.match_indices(&needle).any(|(index, _)| {
        !text[index + needle.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// Ruby's regular expressions call `(?m)` what the `regex` crate calls `(?s)`.
fn matches_pattern(pattern: &str, text: &str) -> Result<bool, String> {
    let pattern = pattern.replace("(?m)", "(?s)");
    regex::Regex::new(&pattern)
        .map(|regex| regex.is_match(text))
        .map_err(|error| error.to_string())
}

fn expected_error(spec: &Json, kind: &str) -> String {
    let line = match spec["errors"]["line"].as_i64() {
        Some(line) => format!(" on line {line}"),
        None => String::new(),
    };
    format!("{kind} matching {:?}{line}", patterns(spec, kind))
}

// --- running a spec ------------------------------------------------------------------------

fn skip_reason(spec: &Json) -> Option<String> {
    let features: Vec<&str> = spec["features"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect();
    for (feature, reason) in SKIPPED_FEATURES {
        if features.contains(feature) {
            return Some(format!("{feature}: {reason}"));
        }
    }
    let modes: Vec<&str> = spec["error_modes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect();
    // A template that is valid in a strict mode is parsed the same way here. One that a strict
    // mode rejects is handed to the legacy parser, which accepts it or reports it in its own
    // words, so what a strict mode raises tells nothing about this engine.
    let strict_only =
        !modes.is_empty() && !modes.iter().any(|mode| matches!(*mode, "lax" | "warn"));
    if strict_only && !spec["errors"]["parse_error"].is_null() {
        return Some("strict parse errors: rejected templates go to the legacy parser".to_string());
    }
    if spec["template"].is_object() {
        return Some("binary_data: byte strings that are not UTF-8".to_string());
    }
    if !spec["template_factory"].is_null() {
        return Some("template_factory: Ruby template factory callbacks".to_string());
    }
    if spec["resource_limits"]
        .as_object()
        .is_some_and(|limits| !limits.is_empty())
    {
        return Some("resource limits".to_string());
    }
    None
}

fn run(env: &Arc<Environment>, spec: &Json) -> Outcome {
    if let Some(reason) = skip_reason(spec) {
        return Outcome::Skip(reason);
    }
    let standard_drops = spec["features"]
        .as_array()
        .is_some_and(|features| features.iter().any(|feature| feature == "drops"));
    let mut environment = Hash::new();
    for (name, value) in spec["environment"].as_object().into_iter().flatten() {
        match to_value(value, standard_drops) {
            Ok(value) => environment.insert(name.clone(), value),
            Err(what) => return Outcome::Skip(format!("environment: needs {what}")),
        };
    }
    let mut files = Vec::new();
    let mut missing = None;
    for (name, source) in spec["filesystem"].as_object().into_iter().flatten() {
        let Some(source) = source.as_str() else {
            return Outcome::Skip("filesystem: a file that is not text".to_string());
        };
        if name == "_error-message" {
            missing = Some(source.to_string());
        } else if name != "instantiate" {
            files.push((name.clone(), source.to_string()));
        }
    }

    let source = spec["template"].as_str().unwrap_or_default();
    let inline = spec["render_errors"].as_bool().unwrap_or(false);
    let expected = spec["expected"].as_str();
    let expected_line = spec["errors"]["line"].as_i64();
    let fail = |expected: String, actual: String| Outcome::Fail { expected, actual };
    let raised = |error: &Error| format!("{}: {error}", class_name(error));
    let matches_raised = |error: &Error, kind: &str| {
        let message = error.to_string();
        all_match(
            &patterns(spec, kind),
            &[&message, core_message(&message), class_name(error)],
        ) && expected_line.is_none_or(|line| error.line.map(i64::from) == Some(line))
    };

    let template = match Template::parse_named(env, source, spec["template_name"].as_str()) {
        Ok(template) => template,
        Err(error) => {
            if !spec["errors"]["parse_error"].is_null() {
                return if matches_raised(&error, "parse_error") {
                    Outcome::Pass
                } else {
                    fail(expected_error(spec, "parse_error"), raised(&error))
                };
            }
            // Where errors are printed, a template that does not parse prints its error.
            let message = error.to_string();
            return match expected {
                Some(expected) if inline && same_output(&message, expected) => Outcome::Pass,
                _ => fail(describe_expectation(spec), raised(&error)),
            };
        }
    };
    if !spec["errors"]["parse_error"].is_null() {
        return fail(
            expected_error(spec, "parse_error"),
            "no error: the template parsed".to_string(),
        );
    }

    let mut ctx = Context::builder(env.clone())
        // The clock of every liquid-spec run.
        .now(Utc.with_ymd_and_hms(2024, 1, 1, 0, 1, 58).unwrap())
        .time_zone(Tz::UTC)
        .globals(Arc::new(StaticEnvironment(environment)))
        .partials(Arc::new(FileSystem {
            env: env.clone(),
            files,
            missing,
        }))
        .build();
    let output = template.render(&mut ctx);

    // The engine prints errors where the reference can also raise them. Where a spec has them
    // raised, the first error recorded is the one the reference would have stopped on.
    if !inline && let Some(error) = ctx.errors().first() {
        if !spec["errors"]["render_error"].is_null() {
            return if matches_raised(error, "render_error") {
                Outcome::Pass
            } else {
                fail(expected_error(spec, "render_error"), raised(error))
            };
        }
        return fail(describe_expectation(spec), raised(error));
    }
    if !spec["errors"]["render_error"].is_null() {
        return fail(
            expected_error(spec, "render_error"),
            format!("no error: rendered {output:?}"),
        );
    }
    if !spec["errors"]["output"].is_null() {
        let found = all_match(&patterns(spec, "output"), &[&output])
            && expected_line.is_none_or(|line| mentions_line(&output, line));
        return if found {
            Outcome::Pass
        } else {
            fail(expected_error(spec, "output"), output)
        };
    }
    if let Some(pattern) = spec["expected_pattern"].as_str() {
        return match matches_pattern(pattern, &output) {
            Ok(true) => Outcome::Pass,
            Ok(false) => fail(format!("output matching /{pattern}/"), output),
            Err(error) => Outcome::Skip(format!("expected_pattern: {error}")),
        };
    }
    match expected {
        Some(expected) if same_output(&output, expected) => Outcome::Pass,
        Some(expected) => fail(expected.to_string(), output),
        None => Outcome::Pass,
    }
}

/// Equal, or two error messages that only differ by their location.
fn same_output(actual: &str, expected: &str) -> bool {
    actual == expected
        || (actual.contains("Liquid error")
            && expected.contains("Liquid error")
            && core_message(actual) == core_message(expected))
}

fn describe_expectation(spec: &Json) -> String {
    match spec["expected"].as_str() {
        Some(expected) => expected.to_string(),
        None => ["render_error", "output"]
            .iter()
            .find(|kind| !spec["errors"][**kind].is_null())
            .map(|kind| expected_error(spec, kind))
            .unwrap_or_else(|| "no error".to_string()),
    }
}

/// Runs a spec on its own thread: a template that recurses needs a deep stack, and one that
/// never ends must not hang the test.
fn run_guarded(env: &Arc<Environment>, spec: &Arc<Json>) -> Outcome {
    let (sender, receiver) = mpsc::channel();
    let thread = {
        let env = env.clone();
        let spec = spec.clone();
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn(move || {
                let _ = sender.send(run(&env, &spec));
            })
            .expect("a thread for the spec")
    };
    match receiver.recv_timeout(TIMEOUT) {
        Ok(outcome) => {
            let _ = thread.join();
            outcome
        }
        Err(mpsc::RecvTimeoutError::Timeout) => Outcome::Fail {
            expected: describe_expectation(spec),
            actual: format!("still rendering after {} seconds", TIMEOUT.as_secs()),
        },
        Err(mpsc::RecvTimeoutError::Disconnected) => Outcome::Fail {
            expected: describe_expectation(spec),
            actual: "the engine panicked".to_string(),
        },
    }
}

// --- the test ------------------------------------------------------------------------------

#[derive(Default)]
struct Tally {
    passed: usize,
    failed: usize,
    skipped: usize,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn clip(text: &str) -> String {
    let mut clipped: String = text.chars().take(400).collect();
    if clipped.len() < text.len() {
        clipped.push('…');
    }
    clipped
}

#[test]
fn matches_liquid_spec() {
    let cases = workspace_root().join(".cache/liquid-spec-cases");
    let Ok(revision) = std::fs::read_to_string(cases.join("REVISION")) else {
        eprintln!("skipped: run `mise run liquid-spec:fetch` to download Shopify's liquid-spec");
        return;
    };
    assert_eq!(
        revision.trim(),
        REVISION.trim(),
        "the downloaded liquid-spec is not the revision the baselines were recorded against: \
         run `mise run liquid-spec:fetch`"
    );

    let env = Arc::new(Environment::standard());
    let only = std::env::var("LIQUID_SPEC_ONLY").ok();
    let verbose = std::env::var("LIQUID_SPEC_VERBOSE").is_ok();
    let mut report = Vec::new();
    let mut tallies: Vec<(&str, Tally)> = Vec::new();
    let mut skip_reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut differences: BTreeMap<String, usize> = BTreeMap::new();
    for (suite, _) in BASELINE {
        let path = cases.join(format!("{suite}.json"));
        let document: Json = serde_json::from_str(
            &std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{}", path.display())),
        )
        .unwrap();
        let mut tally = Tally::default();
        for spec in document["specs"].as_array().unwrap() {
            let name = spec["name"].as_str().unwrap_or_default();
            let file = spec["file"].as_str().unwrap_or_default();
            if only
                .as_ref()
                .is_some_and(|only| !file.contains(only.as_str()) && !name.contains(only.as_str()))
            {
                continue;
            }
            match run_guarded(&env, &Arc::new(spec.clone())) {
                Outcome::Pass => tally.passed += 1,
                Outcome::Skip(reason) => {
                    tally.skipped += 1;
                    let reason = reason.split(':').next().unwrap_or_default().to_string();
                    *skip_reasons.entry(reason).or_default() += 1;
                }
                Outcome::Fail { expected, actual } => {
                    tally.failed += 1;
                    *differences.entry(file.to_string()).or_default() += 1;
                    if verbose {
                        eprintln!(
                            "\n=== {file}:{} {name}\n--- template\n{}\n--- expected\n{}\n--- actual\n{}",
                            spec["line"],
                            clip(spec["template"].as_str().unwrap_or_default()),
                            clip(&expected),
                            clip(&actual)
                        );
                    }
                    report.push(serde_json::json!({
                        "suite": suite,
                        "file": file,
                        "line": spec["line"],
                        "name": name,
                        "template": spec["template"],
                        "environment": spec["environment"],
                        "filesystem": spec["filesystem"],
                        "error_modes": spec["error_modes"],
                        "features": spec["features"],
                        "render_errors": spec["render_errors"],
                        "expected": expected,
                        "actual": actual,
                    }));
                }
            }
        }
        tallies.push((suite, tally));
    }

    eprintln!("liquid-spec {}", &REVISION.trim()[..12]);
    eprintln!(
        "{:<32}{:>8}{:>8}{:>9}",
        "suite", "passed", "failed", "skipped"
    );
    for (suite, tally) in &tallies {
        eprintln!(
            "{suite:<32}{:>8}{:>8}{:>9}",
            tally.passed, tally.failed, tally.skipped
        );
    }
    let total = |count: fn(&Tally) -> usize| tallies.iter().map(|(_, t)| count(t)).sum::<usize>();
    eprintln!(
        "{:<32}{:>8}{:>8}{:>9}",
        "total",
        total(|t| t.passed),
        total(|t| t.failed),
        total(|t| t.skipped)
    );
    let list = |counts: &BTreeMap<String, usize>| {
        counts
            .iter()
            .map(|(name, count)| format!("{name}({count})"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    eprintln!("skipped: {}", list(&skip_reasons));
    eprintln!("differing: {}", list(&differences));
    if let Ok(path) = std::env::var("LIQUID_SPEC_REPORT") {
        std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
        eprintln!("differences written to {path}");
    }

    if only.is_none() {
        for ((suite, baseline), (_, tally)) in BASELINE.iter().zip(&tallies) {
            assert!(
                tally.passed >= *baseline,
                "liquid-spec regressed: {} specs of {suite} pass, the baseline is {baseline}",
                tally.passed
            );
        }
    }
}
