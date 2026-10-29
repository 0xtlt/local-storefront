use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use lsf_core::Theme;
use lsf_core::theme::Revalidate;
use lsf_core::theme::liquid_settings::{holds_settings, string_literal, string_span};
use lsf_liquid::strict2::{self, BROKEN_TODAY, Finding, IssueKind};
use serde::Serialize;

#[derive(Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Format {
    /// For a person: a line per problem, with indented details.
    #[default]
    Text,
    /// One JSON document, for a tool: `{ok, files, problems, fixable, fixed, changes_output,
    /// diagnostics: [{severity, code, file, line, message, ...}]}`.
    Json,
}

#[derive(clap::Args)]
pub struct Args {
    /// Rewrites the markup Shopify's strict Liquid parser rejects into markup it accepts: what
    /// the code was meant to do where that is clear (a filter in a `render` argument applied,
    /// an `and` evaluated), and what renders the same as today elsewhere. The report says
    /// where the output changes.
    #[arg(long)]
    fix: bool,

    /// How to print the result.
    #[arg(long, value_enum, default_value = "text")]
    format: Format,
}

/// What `lsf check` writes into a file.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Written {
    Nothing,
    /// What the code meant where that is clear, the same output elsewhere.
    Fixes,
    /// Only the fixes that keep the output: what the code meant did not check out.
    SameOutput,
}

impl Args {
    fn writes(&self) -> Written {
        if self.fix {
            Written::Fixes
        } else {
            Written::Nothing
        }
    }
}

/// Whether `snippets/<snippet>.liquid` may read `variable`: a snippet that cannot be read may.
fn snippet_reads(theme: &Theme, snippet: &str, variable: &str) -> bool {
    theme
        .files()
        .read(&format!("snippets/{snippet}.liquid"))
        .is_none_or(|source| strict2::mentions(&source, variable))
}

/// The findings of a piece of Liquid to report, with the fixes that leave out what snippets do
/// not read.
fn findings_of(env: &lsf_liquid::Environment, theme: &Theme, source: &str) -> Vec<Finding> {
    let mut findings = reportable(strict2::audit(env, source).findings);
    strict2::drop_unread(&mut findings, |snippet, variable| {
        snippet_reads(theme, snippet, variable)
    });
    findings
}

/// A piece of Liquid rewritten as `mode` asks, and what was written into it. Intents that do
/// not check out fall back on the fixes, with a warning at `place`.
fn rewrite(
    env: &lsf_liquid::Environment,
    theme: &Theme,
    source: &str,
    findings: &[Finding],
    mode: Written,
    place: Location,
    tally: &mut Tally,
) -> (String, Written) {
    match mode {
        Written::Nothing => (source.to_string(), Written::Nothing),
        Written::SameOutput => (strict2::apply(source, findings), Written::SameOutput),
        Written::Fixes => match strict2::apply_intents(env, source, |snippet, variable| {
            snippet_reads(theme, snippet, variable)
        }) {
            Ok(rewritten) => (rewritten, Written::Fixes),
            Err(reason) => {
                tally.report(Diagnostic::new(
                    "warning",
                    "fix_fallback",
                    place,
                    format!("writes only the fixes that keep the output: {reason}"),
                ));
                (strict2::apply(source, findings), Written::SameOutput)
            }
        },
    }
}

/// Tags whose body is not Liquid.
const RAW_TAGS: [&str; 6] = [
    "schema",
    "raw",
    "javascript",
    "stylesheet",
    "doc",
    "comment",
];

/// The filter names after each `|` of one piece of markup, outside string literals.
fn filters_in(markup: &str, line: usize, found: &mut Vec<(String, usize)>) {
    let mut quote: Option<char> = None;
    for (index, c) in markup.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '|' => {
                let name: String = markup[index + 1..]
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    found.push((name, line + markup[..index].matches('\n').count()));
                }
            }
            None => {}
        }
    }
}

/// The filter names used in a Liquid source, with the line each one is on.
fn filters_used(source: &str) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    let mut position = 0;
    while let Some(start) = [source[position..].find("{{"), source[position..].find("{%")]
        .into_iter()
        .flatten()
        .min()
        .map(|start| position + start)
    {
        let output = source[start..].starts_with("{{");
        let Some(length) = source[start..].find(if output { "}}" } else { "%}" }) else {
            break;
        };
        let markup = &source[start + 2..start + length];
        let line = source[..start].matches('\n').count() + 1;
        position = start + length + 2;
        if output {
            filters_in(markup, line, &mut found);
            continue;
        }
        let tag = markup.trim_matches('-').trim();
        let name = tag.split_whitespace().next().unwrap_or_default();
        if tag.starts_with('#') {
            continue;
        }
        if RAW_TAGS.contains(&name) {
            // Skip to the end tag: what is in between is not Liquid.
            position = source[position..]
                .find(&format!("end{name}"))
                .map_or(source.len(), |index| position + index);
            continue;
        }
        if name != "liquid" {
            filters_in(markup, line, &mut found);
            continue;
        }
        // `{% liquid %}` holds one tag per line, comments included.
        let mut comment_depth = 0;
        for (offset, statement) in markup.lines().enumerate() {
            let statement = statement.trim();
            let name = statement.split_whitespace().next().unwrap_or_default();
            match name {
                "comment" => comment_depth += 1,
                "endcomment" if comment_depth > 0 => comment_depth -= 1,
                _ if comment_depth > 0 || statement.starts_with('#') => {}
                _ => filters_in(statement, line + offset, &mut found),
            }
        }
    }
    found
}

/// For an unknown tag that a feature preview has, how to turn the preview on.
fn preview_hint(error: &str) -> String {
    let Some(tag) = error
        .split("Unknown tag '")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
    else {
        return String::new();
    };
    match lsf_core::Preview::of_tag(tag) {
        Some(preview) => format!(
            " (`{tag}` is part of Shopify's \"{}\" feature preview: run with `--preview {}`)",
            preview.title(),
            preview.name()
        ),
        None => String::new(),
    }
}

/// The line of the first `{% block %}` call of a file, outside `doc`, `comment` and `raw`,
/// where examples of calls are written. The statements of `{% liquid %}` count too; what is
/// inside `{{ }}` does not.
fn block_call_line(source: &str) -> Option<usize> {
    let line_at = |index: usize| source[..index].matches('\n').count() + 1;
    let mut quiet: Option<&str> = None;
    // Whether a statement opens a call, given the name of its tag; updates `quiet`.
    let calls = |name: &str, quiet: &mut Option<&str>| -> bool {
        match *quiet {
            Some(end) if name == end => *quiet = None,
            Some(_) => {}
            None => match name {
                "doc" => *quiet = Some("enddoc"),
                "comment" => *quiet = Some("endcomment"),
                "raw" => *quiet = Some("endraw"),
                "block" => return true,
                _ => {}
            },
        }
        false
    };
    let mut position = 0;
    while let Some(open) = source[position..].find('{') {
        let start = position + open;
        let after = &source[start + 1..];
        if after.starts_with('{') && quiet.is_none() {
            // An output: skipped whole, strings and all.
            position = start + 2 + after[1..].find("}}").map_or(after.len() - 1, |end| end + 2);
            continue;
        }
        if !after.starts_with('%') {
            position = start + 1;
            continue;
        }
        let close = after.find("%}")?;
        let markup = after[1..close].trim_matches('-').trim();
        let name = markup.split_whitespace().next().unwrap_or_default();
        if name == "liquid" && quiet.is_none() {
            let body_start = start + 2 + after[1..].find("liquid").unwrap_or(0) + "liquid".len();
            for (offset, statement) in source[body_start..start + 1 + close].lines().enumerate() {
                let word = statement.split_whitespace().next().unwrap_or_default();
                if calls(word, &mut quiet) {
                    return Some(line_at(body_start) + offset);
                }
            }
            quiet = None;
        } else if calls(name, &mut quiet) {
            return Some(line_at(start));
        }
        position = start + 1 + close + 2;
    }
    None
}

/// Markup on one line, for a report: its whitespace runs become one space.
fn one_line(markup: &str) -> String {
    markup.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The words where `was` and `now` differ, with a word around them: what a report shows of a
/// long markup.
fn changed_words(was: &str, now: &str) -> (String, String) {
    let was: Vec<&str> = was.split_whitespace().collect();
    let now: Vec<&str> = now.split_whitespace().collect();
    let prefix = was.iter().zip(&now).take_while(|(a, b)| a == b).count();
    let suffix = was[prefix..]
        .iter()
        .rev()
        .zip(now[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let show = |words: &[&str]| {
        let start = prefix.saturating_sub(1);
        let end = (words.len() - suffix + 1).min(words.len());
        let mut out = words[start..end].join(" ");
        if start > 0 {
            out.insert_str(0, "… ");
        }
        if end < words.len() {
            out.push_str(" …");
        }
        out
    };
    (show(&was), show(&now))
}

/// How the report names what a finding is about: `{% render %}` or `{{ }}`.
fn subject(finding: &Finding) -> String {
    match &finding.tag {
        Some(tag) => format!("{{% {tag} %}}"),
        None => "{{ }}".to_string(),
    }
}

/// Where a problem is: a line of a file, or of the value of a `liquid` setting in a JSON file.
#[derive(Clone, Serialize)]
struct Location {
    file: String,
    /// The line in the file. For a setting, the line its value starts on.
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    /// The setting, as the keys that lead to it joined with dots.
    #[serde(skip_serializing_if = "Option::is_none")]
    setting: Option<String>,
    /// The line in the value of the setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    setting_line: Option<u32>,
}

impl Location {
    fn file(file: &str, line: Option<usize>) -> Location {
        Location {
            file: file.to_string(),
            line,
            setting: None,
            setting_line: None,
        }
    }
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.file)?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        if let Some(setting) = &self.setting {
            write!(f, ": {setting}")?;
        }
        match self.setting_line {
            Some(line) if line > 1 => write!(f, ", line {line}"),
            _ => Ok(()),
        }
    }
}

/// A problem `lsf check` reports.
#[derive(Serialize)]
struct Diagnostic {
    /// `error` counts as a problem; `warning` does not.
    severity: &'static str,
    code: &'static str,
    #[serde(flatten)]
    location: Location,
    message: String,
    /// What Shopify's strict parser does not read: `{% render %}`, or `{{ }}` for an output.
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<String>,
    /// The markup as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    markup: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<Fix>,
    /// Why there is no fix.
    #[serde(skip_serializing_if = "Option::is_none")]
    no_fix: Option<String>,
}

impl Diagnostic {
    fn new(
        severity: &'static str,
        code: &'static str,
        location: Location,
        message: String,
    ) -> Diagnostic {
        Diagnostic {
            severity,
            code,
            location,
            message,
            subject: None,
            markup: None,
            fix: None,
            no_fix: None,
        }
    }

    /// Prints the diagnostic for a person.
    fn print(&self) {
        let Some(markup) = &self.markup else {
            println!("{}: {}", self.location, self.message);
            return;
        };
        let written = self.fix.as_ref().is_some_and(|fix| fix.written);
        let fixed = if written { "fixed: " } else { "" };
        println!("{}: {fixed}{}", self.location, self.message);
        let Some(fix) = &self.fix else {
            println!("  was: {}", one_line(markup));
            if let Some(reason) = &self.no_fix {
                println!("  no automatic fix: {reason}");
            }
            return;
        };
        let label = if written { "now" } else { "fix" };
        // A tag that replaces this one whole is shown whole.
        let (was, now) = if fix.markup.starts_with("{%") {
            (one_line(markup), one_line(&fix.markup))
        } else {
            changed_words(markup, &fix.markup)
        };
        println!("  was: {was}");
        println!("  {label}: {now}");
        if !fix.before.is_empty() {
            println!("  before it: {}", fix.before.join("; "));
        }
        if let Some(condition) = &fix.body_in_if {
            println!("  its body goes in: {{% if {condition} %}}");
        }
        if let Some(left_out) = &fix.left_out {
            println!(
                "  left out: {} (read out of a filter, passed to {}, which does not read it)",
                left_out
                    .variables
                    .iter()
                    .map(|variable| format!("`{variable}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                left_out.snippet
            );
        }
        if let Some(change) = &fix.changes_output {
            println!("  changes the output: {change}");
        }
    }
}

/// The fix of a problem.
#[derive(Serialize)]
struct Fix {
    /// Whether `--fix` wrote it.
    written: bool,
    /// What replaces the markup; a whole tag when the fix rewrites the tags around it.
    markup: String,
    /// Tags put on the lines above, without their delimiters.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    before: Vec<String>,
    /// The condition of the `if` the body of a `when` goes in.
    #[serde(skip_serializing_if = "Option::is_none")]
    body_in_if: Option<String>,
    /// Arguments left out of a `render`, which the snippet does not read.
    #[serde(skip_serializing_if = "Option::is_none")]
    left_out: Option<LeftOut>,
    /// What renders differently with the fix. Absent when the output stays the same.
    #[serde(skip_serializing_if = "Option::is_none")]
    changes_output: Option<String>,
}

#[derive(Serialize)]
struct LeftOut {
    variables: Vec<String>,
    snippet: String,
}

/// What the check found, and what `--fix` wrote.
#[derive(Default)]
struct Tally {
    problems: usize,
    fixable: usize,
    /// Fixes that change the output, where the code now does what it says.
    changes: usize,
    fixed: usize,
    /// Kept for the JSON report; printed at once for a person.
    json: bool,
    diagnostics: Vec<Diagnostic>,
}

impl Tally {
    fn report(&mut self, diagnostic: Diagnostic) {
        if self.json {
            self.diagnostics.push(diagnostic);
        } else {
            diagnostic.print();
        }
    }

    /// An error that has no fix.
    fn problem(&mut self, code: &'static str, location: Location, message: String) {
        self.problems += 1;
        self.report(Diagnostic::new("error", code, location, message));
    }

    /// Reports what Shopify's strict parser does not read in a piece of Liquid (`source`): a
    /// file, or the value of a setting. `location` says where a finding is; `written`, what was
    /// written into it.
    fn strict2(
        &mut self,
        findings: &[Finding],
        source: &str,
        written: Written,
        location: impl Fn(&Finding) -> Location,
    ) {
        for finding in findings {
            let (code, message) = match finding.issue.kind {
                IssueKind::Rejected => (
                    "strict_liquid_rejected",
                    format!(
                        "Shopify's strict Liquid parser rejects this {}: {}",
                        subject(finding),
                        finding.issue.message
                    ),
                ),
                IssueKind::Reinterpreted => (
                    "strict_liquid_reinterpreted",
                    format!(
                        "Shopify's strict Liquid parser reads this {} differently",
                        subject(finding)
                    ),
                ),
            };
            let mut diagnostic = Diagnostic::new("error", code, location(finding), message);
            diagnostic.subject = Some(subject(finding));
            diagnostic.markup = Some(finding.markup(source).to_string());
            // An intent needs what it writes around the tag: the tag, or the body that follows.
            let intent = finding
                .issue
                .intent
                .as_ref()
                .filter(|intent| strict2::applies(intent, finding));
            let fixed = written != Written::Nothing;
            diagnostic.fix = match (&finding.issue.fix, intent) {
                // What the code meant, unless only the fixes that keep the output were written.
                (_, Some(intent)) if written != Written::SameOutput => {
                    // A rewrite of the block gives the tag that replaces this one.
                    let markup = intent
                        .block
                        .iter()
                        .find(|(range, _)| {
                            range.start <= finding.span.start && range.end >= finding.span.end
                        })
                        .map_or(&intent.markup, |(_, text)| text);
                    Some(Fix {
                        written: fixed,
                        markup: markup.clone(),
                        before: intent.prelude.clone(),
                        body_in_if: intent.body_condition.clone(),
                        left_out: None,
                        changes_output: Some(intent.change.clone()),
                    })
                }
                (Ok(fix), _) => Some(Fix {
                    written: fixed,
                    markup: fix.clone(),
                    before: Vec::new(),
                    body_in_if: None,
                    // Arguments the lax parser passed by mistake, which the fix leaves out.
                    left_out: finding
                        .issue
                        .strays
                        .as_ref()
                        .filter(|strays| fix == &strays.fix)
                        .map(|strays| LeftOut {
                            variables: strays.variables.clone(),
                            snippet: format!("snippets/{}.liquid", strays.snippet),
                        }),
                    changes_output: finding.issue.changes.clone(),
                }),
                (Err(reason), _) => {
                    diagnostic.no_fix = Some(reason.clone());
                    None
                }
            };
            match &diagnostic.fix {
                Some(fix) => self.count(fix.written, fix.changes_output.is_some()),
                None => self.problems += 1,
            }
            self.report(diagnostic);
        }
    }

    /// Counts a problem that has a fix: written (`fixed`) or not, and whether it changes the
    /// output.
    fn count(&mut self, fixed: bool, changes: bool) {
        if fixed {
            self.fixed += 1;
        } else {
            self.problems += 1;
            self.fixable += 1;
        }
        if changes {
            self.changes += 1;
        }
    }
}

/// The findings to report: markup that does not parse today is reported as the parse error it
/// is.
fn reportable(findings: Vec<Finding>) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|finding| !matches!(&finding.issue.fix, Err(reason) if reason == BROKEN_TODAY))
        .collect()
}

/// Writes `content` to `file` if the file still holds what was `read`.
fn write_if_unchanged(file: Option<PathBuf>, read: &str, content: &str) -> bool {
    file.is_some_and(|file| {
        std::fs::read_to_string(&file).is_ok_and(|on_disk| on_disk == read)
            && std::fs::write(&file, content).is_ok()
    })
}

/// Checks the Liquid of the `liquid` settings of a JSON file against Shopify's strict parser,
/// and writes into the file what `mode` asks for.
fn check_liquid_settings(
    theme: &Theme,
    env: &lsf_liquid::Environment,
    path: &str,
    mode: Written,
    tally: &mut Tally,
) {
    let Some(source) = theme.files().read(path) else {
        return;
    };
    let mut checked = Vec::new();
    let mut replacements: Vec<(Range<usize>, String)> = Vec::new();
    for setting in theme.liquid_settings(path) {
        let findings = findings_of(env, theme, &setting.value);
        if findings.is_empty() {
            continue;
        }
        let span = string_span(&source, &setting.path);
        let place = Location {
            file: path.to_string(),
            line: span
                .as_ref()
                .map(|span| source[..span.start].matches('\n').count() + 1),
            setting: Some(setting.dotted()),
            setting_line: None,
        };
        let (fixed, wrote) = rewrite(env, theme, &setting.value, &findings, mode, place, tally);
        if let Some(span) = &span
            && fixed != setting.value
        {
            replacements.push((span.clone(), string_literal(&fixed, &source[span.clone()])));
        }
        checked.push((setting, findings, span, wrote));
    }
    replacements.sort_by_key(|(span, _)| span.start);
    let mut fixed_source = String::with_capacity(source.len());
    let mut position = 0;
    for (span, literal) in &replacements {
        fixed_source.push_str(&source[position..span.start]);
        fixed_source.push_str(literal);
        position = span.end;
    }
    fixed_source.push_str(&source[position..]);
    let written = mode != Written::Nothing
        && !replacements.is_empty()
        && lsf_core::json::parse_lenient(path, &fixed_source).is_ok()
        && write_if_unchanged(theme.files().resolve(path), &source, &fixed_source);
    for (setting, findings, span, wrote) in checked {
        let line = span.map_or(1, |span| source[..span.start].matches('\n').count() + 1);
        let dotted = setting.dotted();
        let wrote = if written { wrote } else { Written::Nothing };
        tally.strict2(&findings, &setting.value, wrote, |finding| Location {
            file: path.to_string(),
            line: Some(line),
            setting: Some(dotted.clone()),
            setting_line: Some(finding.line),
        });
    }
}

/// The JSON report of `--format json`.
fn json_report(files: usize, tally: &Tally) -> serde_json::Value {
    serde_json::json!({
        "ok": tally.problems == 0,
        "files": files,
        "problems": tally.problems,
        "fixable": tally.fixable,
        "fixed": tally.fixed,
        "changes_output": tally.changes,
        "diagnostics": tally.diagnostics,
    })
}

pub fn run(
    theme_dir: &Path,
    previews: &[lsf_core::Preview],
    args: Args,
) -> Result<ExitCode, String> {
    let (files, tally) = check_theme(theme_dir, previews, &args)?;
    if tally.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json_report(files, &tally))
                .map_err(|error| error.to_string())?
        );
    } else {
        if tally.fixed > 0 {
            eprintln!(
                "{files} file(s) checked, {} problem(s), {} fixed",
                tally.problems, tally.fixed
            );
        } else {
            eprintln!("{files} file(s) checked, {} problem(s)", tally.problems);
        }
        if tally.fixable > 0 {
            eprintln!(
                "{} of them can be fixed: run again with `--fix`",
                tally.fixable
            );
        }
        if tally.changes > 0 {
            eprintln!(
                "{} of the fixes change the output, where the code now does what it says: review the diff",
                tally.changes
            );
        }
    }
    Ok(if tally.problems == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Checks the theme: how many files were read, and what was found.
fn check_theme(
    theme_dir: &Path,
    previews: &[lsf_core::Preview],
    args: &Args,
) -> Result<(usize, Tally), String> {
    let env = Arc::new(lsf_core::environment_with(previews));
    let theme = Theme::open(theme_dir, env.clone(), Revalidate::Never)
        .map_err(|error| error.to_string())?;
    if args.format != Format::Json {
        crate::app::note_classic_accounts(&theme);
    }
    let mut files = 0;
    let mut tally = Tally {
        json: args.format == Format::Json,
        ..Tally::default()
    };
    for directory in [
        "layout",
        "templates",
        "templates/customers",
        "sections",
        "blocks",
        "snippets",
    ] {
        for name in theme.files().list(directory).iter() {
            if !name.ends_with(".liquid") {
                continue;
            }
            files += 1;
            let path = format!("{directory}/{name}");
            let source = theme.files().read(&path).unwrap_or_default();
            let findings = findings_of(&env, &theme, &source);
            let (fixed_source, wrote) = rewrite(
                &env,
                &theme,
                &source,
                &findings,
                args.writes(),
                Location::file(&path, None),
                &mut tally,
            );
            let written = if fixed_source != *source
                && write_if_unchanged(theme.files().resolve(&path), &source, &fixed_source)
            {
                wrote
            } else {
                Written::Nothing
            };
            tally.strict2(&findings, &source, written, |finding| {
                Location::file(&path, Some(finding.line as usize))
            });
            if let Err(error) = theme.liquid(&path) {
                let message = format!("{error}{}", preview_hint(&error.to_string()));
                let line = error.line.map(|line| line as usize);
                tally.problem("liquid_syntax", Location::file(&path, line), message);
                continue;
            }
            if env.tag("block").is_some()
                && !matches!(directory, "layout" | "templates" | "templates/customers")
                && let Some(line) = block_call_line(&source)
            {
                tally.problem(
                    "block_outside_template",
                    Location::file(&path, Some(line)),
                    "the block tag can only be used in layout/ and templates/ files".to_string(),
                );
            }
            let mut reported = Vec::new();
            for (filter, line) in filters_used(&source) {
                if env.filter(&filter).is_none() && !reported.contains(&filter) {
                    tally.problem(
                        "unknown_filter",
                        Location::file(&path, Some(line)),
                        format!(
                            "unknown filter `{filter}` (Shopify ignores unknown filters; this one is not implemented here)"
                        ),
                    );
                    reported.push(filter);
                }
            }
        }
    }
    for directory in [
        "templates",
        "templates/customers",
        "sections",
        "config",
        "locales",
    ] {
        for name in theme.files().list(directory).iter() {
            if !name.ends_with(".json") {
                continue;
            }
            files += 1;
            let path = format!("{directory}/{name}");
            if let Err(error) = theme.json(&path) {
                let message = match error {
                    lsf_core::Error::Json { message, .. } => message,
                    other => other.to_string(),
                };
                tally.problem("invalid_json", Location::file(&path, None), message);
                continue;
            }
            if holds_settings(&path) {
                check_liquid_settings(&theme, &env, &path, args.writes(), &mut tally);
            }
        }
    }
    Ok((files, tally))
}

#[cfg(test)]
mod tests {
    use super::{
        Args, Format, block_call_line, changed_words, check_theme, filters_used, json_report,
        preview_hint, run,
    };

    #[test]
    fn a_report_shows_the_words_that_change() {
        assert_eq!(
            changed_words("'a', b: c and d, e: f", "'a', b: c, e: f"),
            ("… b: c and d, e: …".to_string(), "… b: c, e: …".to_string())
        );
        assert_eq!(
            changed_words("a b", "a"),
            ("a b".to_string(), "a".to_string())
        );
    }

    #[test]
    fn the_json_report_says_what_the_text_says() {
        let root = std::env::temp_dir().join(format!("lsf-check-json-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, content: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write("layout/theme.liquid", "{{ content_for_layout }}");
        write("snippets/image.liquid", "{{ alt }}");
        write(
            "snippets/card.liquid",
            "<p>\n{% render 'image', alt: a | upcase %}\n{{ x y }}\n{% if a = b %}{% endif %}\n{{ z | nope }}",
        );
        write("snippets/broken.liquid", "\n{% if %}");
        let report = |fix: bool| {
            let args = Args {
                fix,
                format: Format::Json,
            };
            let (files, tally) = check_theme(&root, &[], &args).unwrap();
            json_report(files, &tally)
        };
        let before = report(false);
        assert_eq!(before["ok"], false);
        assert_eq!(before["files"], 4);
        // The filter, the output, the operator; the syntax error; the unknown filter.
        assert_eq!(before["problems"], 5);
        assert_eq!(before["fixable"], 3);
        assert_eq!(before["changes_output"], 2);
        let diagnostics = before["diagnostics"].as_array().unwrap();
        let find = |code: &str, line: u64| {
            diagnostics
                .iter()
                .find(|d| d["code"] == code && d["line"] == line)
                .unwrap_or_else(|| panic!("no {code} on line {line}: {diagnostics:?}"))
                .clone()
        };
        let render = find("strict_liquid_rejected", 2);
        assert_eq!(render["file"], "snippets/card.liquid");
        assert_eq!(render["subject"], "{% render %}");
        assert_eq!(render["markup"], "'image', alt: a | upcase");
        assert_eq!(
            render["fix"],
            serde_json::json!({
                "written": false,
                "markup": "'image', alt: image_alt",
                "before": ["assign image_alt = a | upcase"],
                "changes_output": "`alt` gets `a | upcase`",
            })
        );
        assert_eq!(find("strict_liquid_rejected", 3)["fix"]["markup"], "x");
        assert!(find("strict_liquid_rejected", 3)["fix"]["changes_output"].is_null());
        assert_eq!(find("liquid_syntax", 2)["file"], "snippets/broken.liquid");
        assert_eq!(find("unknown_filter", 5)["severity"], "error");
        let after = report(true);
        assert_eq!(after["fixed"], 3);
        assert_eq!(after["fixable"], 0);
        assert_eq!(after["problems"], 2);
        assert_eq!(
            after["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| d["fix"]["written"] == true)
                .count(),
            3
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn fix_rewrites_files_and_liquid_settings() {
        let root = std::env::temp_dir().join(format!("lsf-check-fix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, content: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write("layout/theme.liquid", "{{ content_for_layout }}");
        write(
            "snippets/card.liquid",
            "{% render 'price',\n  price: p,\n  on_sale: a and b\n%}",
        );
        write("snippets/price.liquid", "{{ price }}");
        write(
            "snippets/filtered.liquid",
            "{% render 'price', price: p | default: 0 %}",
        );
        write(
            "sections/custom.liquid",
            "{{ section.settings.code }}{% schema %}{\"settings\": [{\"type\": \"liquid\", \"id\": \"code\"}]}{% endschema %}",
        );
        write(
            "templates/index.json",
            "/* edited by Shopify */\n{\"sections\": {\"a\": {\"type\": \"custom\", \"settings\": {\"code\": \"\\u003cb\\u003e{{ x y }}\"}}}, \"order\": [\"a\"]}",
        );
        let check = |fix: bool| {
            run(
                &root,
                &[],
                Args {
                    fix,
                    format: Format::Text,
                },
            )
            .unwrap()
        };
        assert_eq!(check(false), std::process::ExitCode::FAILURE);
        check(true);
        let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
        // What the code meant: the filter applies, the condition is evaluated.
        assert_eq!(
            read("snippets/filtered.liquid"),
            "{% assign price_value = p | default: 0 -%}\n{% render 'price', price: price_value %}"
        );
        assert_eq!(
            read("snippets/card.liquid"),
            "{% liquid\n  assign price_on_sale = false\n  if a and b\n    assign price_on_sale = true\n  endif\n-%}\n{% render 'price',\n  price: p,\n  on_sale: price_on_sale\n%}"
        );
        // Elsewhere, the same output.
        assert_eq!(
            read("templates/index.json"),
            "/* edited by Shopify */\n{\"sections\": {\"a\": {\"type\": \"custom\", \"settings\": {\"code\": \"\\u003cb\\u003e{{ x }}\"}}}, \"order\": [\"a\"]}"
        );
        assert_eq!(check(false), std::process::ExitCode::SUCCESS);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn block_calls_are_found_outside_documentation() {
        assert_eq!(
            block_call_line("{% doc %}\n{% block 'a' %}{% endblock %}\n{% enddoc %}<p>"),
            None
        );
        assert_eq!(
            block_call_line("{% comment %}{% block 'a' %}{% endcomment %}\n\n{%- block 'b' -%}"),
            Some(3)
        );
        assert_eq!(block_call_line("{% render 'block' %}"), None);
        // An output that holds `{%` is not a tag.
        assert_eq!(
            block_call_line("{{ \"{% block 'x' %}\" }}\n{% comment %}{% endcomment %}"),
            None
        );
        // A statement of `{% liquid %}`.
        assert_eq!(
            block_call_line("<p>\n{% liquid\n  assign a = 1\n  block 'a'\n  endblock\n%}"),
            Some(4)
        );
        assert_eq!(
            block_call_line("{% liquid\n  comment\n  block 'a'\n  endcomment\n%}"),
            None
        );
    }

    #[test]
    fn a_tag_of_a_preview_names_the_preview() {
        assert_eq!(
            preview_hint("Liquid syntax error (templates/index line 1): Unknown tag 'block'"),
            " (`block` is part of Shopify's \"Liquid July '26 changes\" feature preview: run with `--preview liquid-july-26`)"
        );
        assert_eq!(preview_hint("Liquid syntax error: Unknown tag 'nope'"), "");
        assert_eq!(preview_hint("Liquid syntax error: something else"), "");
    }

    fn names(source: &str) -> Vec<(String, usize)> {
        filters_used(source)
    }

    #[test]
    fn finds_filters_with_their_line() {
        assert_eq!(
            names("{{ a | upcase }}\n{% assign b = a | plus: 1 | minus: 2 %}"),
            vec![
                ("upcase".to_string(), 1),
                ("plus".to_string(), 2),
                ("minus".to_string(), 2)
            ]
        );
        assert_eq!(
            names("{{ 'a | b' | append: \"|c\" }}"),
            vec![("append".to_string(), 1)]
        );
    }

    #[test]
    fn ignores_what_is_not_liquid() {
        assert!(names("{% comment %} a | b {% endcomment %}").is_empty());
        assert!(names("{%- comment -%} a | b {%- endcomment -%}").is_empty());
        assert!(
            names("{% # a | b %}{% schema %}{\"a\": \"{{ x | y }}\"}{% endschema %}").is_empty()
        );
        assert!(names("{% raw %}{{ a | b }}{% endraw %}").is_empty());
    }

    #[test]
    fn reads_liquid_tags_line_by_line() {
        let source = "{%- liquid\n  comment\n    a || b\n  endcomment\n  # c | d\n  assign e = f | size\n-%}";
        assert_eq!(names(source), vec![("size".to_string(), 6)]);
    }
}
