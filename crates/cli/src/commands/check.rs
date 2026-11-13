use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use lsf_core::theme::Revalidate;
use lsf_liquid::strict2::{self, BROKEN_TODAY, Finding, IssueKind};
use lsf_theme::Theme;
use lsf_theme::theme::liquid_settings::{holds_settings, string_literal, string_span};
use serde::Serialize;

use crate::output::count;

mod placement;
mod references;

#[derive(Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Format {
    /// For a person: a line per problem, with indented details.
    #[default]
    Text,
    /// One JSON document, for a tool: `{ok, files, problems, fixable, fixed, changes_output,
    /// diagnostics: [{severity, code, file, line, message, ...}]}`.
    Json,
    /// GitHub Actions annotations, one per problem (`::error file=…,line=…::message`): a
    /// workflow shows each on its line of the pull request.
    Github,
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

    /// The directory of a Shopify app: the app blocks the theme names are checked against its
    /// theme app extensions. Without it, they are not checked, and a note says how many there
    /// are.
    #[arg(long, value_name = "DIR")]
    app: Option<PathBuf>,
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
/// Filters of other template languages, and the filter of Shopify that does the same: a theme
/// written from memory uses them.
const OTHER_FILTERS: &[(&str, &str)] = &[
    ("uppercase", "upcase"),
    ("upper", "upcase"),
    ("lowercase", "downcase"),
    ("lower", "downcase"),
    ("length", "size"),
    ("trim", "strip"),
    ("ltrim", "lstrip"),
    ("rtrim", "rstrip"),
    ("json_encode", "json"),
];

/// The filter an unknown one probably is: a filter (or a name of [`OTHER_FILTERS`]) one or two
/// edits away, one for a short name, or with the same letters. The closest wins.
fn meant_filter(env: &lsf_liquid::Environment, unknown: &str) -> Option<String> {
    let letters = |name: &str| {
        let mut letters: Vec<char> = name.chars().collect();
        letters.sort_unstable();
        letters
    };
    let mut names = env.filter_names();
    names.sort_unstable();
    let known = names.into_iter().map(|name| (name, name));
    let other = OTHER_FILTERS.iter().copied();
    let most = if unknown.chars().count() < 5 { 1 } else { 2 };
    known
        .chain(other)
        .filter_map(|(name, filter)| {
            let distance = lsf_core::util::edit_distance(unknown, name);
            let close = distance <= most || letters(unknown) == letters(name);
            close.then_some((distance, filter))
        })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, filter)| filter.to_string())
}

fn preview_hint(error: &str) -> String {
    let Some(tag) = error
        .split("Unknown tag '")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
    else {
        return String::new();
    };
    match lsf_theme::Preview::of_tag(tag) {
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
    /// What was probably meant: `did you mean "upcase"?`.
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
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
            hint: None,
        }
    }

    /// The diagnostic as a GitHub Actions annotation, its file in `base`.
    fn annotation(&self, base: &Path) -> crate::output::Annotation {
        let mut message = self.message.clone();
        if let Some(setting) = &self.location.setting {
            message = format!("{setting}: {message}");
        }
        if let Some(fix) = &self.fix {
            message.push_str(&format!("\nfix: {}", one_line(&fix.markup)));
            if !fix.before.is_empty() {
                message.push_str(&format!("\nbefore it: {}", fix.before.join("; ")));
            }
            if let Some(change) = &fix.changes_output {
                message.push_str(&format!("\nchanges the output: {change}"));
            }
        }
        if let Some(reason) = &self.no_fix {
            message.push_str(&format!("\nno automatic fix: {reason}"));
        }
        if let Some(hint) = &self.hint {
            message.push_str(&format!("\nhint: {hint}"));
        }
        crate::output::Annotation {
            level: crate::output::Annotation::level_of(self.severity),
            file: Some(crate::output::annotation_path(base, &self.location.file)),
            line: self.location.line,
            title: format!("lsf check: {}", self.code),
            message,
        }
    }

    /// Prints the diagnostic for a person.
    fn print(&self) {
        // A warning says so: it is not one of the problems the summary counts.
        let warning = match self.severity {
            "warning" => "warning: ",
            "info" => "note: ",
            _ => "",
        };
        let Some(markup) = &self.markup else {
            println!("{}: {warning}{}", self.location, self.message);
            if let Some(hint) = &self.hint {
                println!("  hint: {hint}");
            }
            return;
        };
        let written = self.fix.as_ref().is_some_and(|fix| fix.written);
        let fixed = if written { "fixed: " } else { "" };
        println!("{}: {warning}{fixed}{}", self.location, self.message);
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
    /// For `--format github`: the theme folder, which the files of the annotations are in.
    github: Option<PathBuf>,
    diagnostics: Vec<Diagnostic>,
}

impl Tally {
    fn report(&mut self, diagnostic: Diagnostic) {
        if self.json {
            self.diagnostics.push(diagnostic);
        } else if let Some(base) = &self.github {
            println!("{}", diagnostic.annotation(base));
        } else {
            diagnostic.print();
        }
    }

    /// An error that has no fix.
    fn problem(&mut self, code: &'static str, location: Location, message: String) {
        self.problem_with_hint(code, location, message, None);
    }

    /// An error that has no fix, with what was probably meant.
    fn problem_with_hint(
        &mut self,
        code: &'static str,
        location: Location,
        message: String,
        hint: Option<String>,
    ) {
        self.problems += 1;
        let mut diagnostic = Diagnostic::new("error", code, location, message);
        diagnostic.hint = hint;
        self.report(diagnostic);
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

/// A warning for a file that is not UTF-8 (saved as Latin-1 or Windows-1252): Shopify takes
/// it, and each byte that is not UTF-8 shows as U+FFFD, as lsf reads it.
fn report_encoding(theme: &Theme, path: &str, tally: &mut Tally) {
    let Some(line) = theme
        .files()
        .bytes(path)
        .and_then(|bytes| lsf_core::theme::first_non_utf8_line(&bytes))
    else {
        return;
    };
    let mut diagnostic = Diagnostic::new(
        "warning",
        "not_utf8",
        Location::file(path, Some(line)),
        "the file is not UTF-8: Shopify takes it, and each byte that is not UTF-8 shows as \u{FFFD} (`caf\u{FFFD}` for a Latin-1 `café`)".to_string(),
    );
    diagnostic.hint = Some("save the file as UTF-8".to_string());
    tally.report(diagnostic);
}

/// The JSON report of `--format json`.
/// A contextual template or section group (`index.context.europe.json`): a warning when lsf
/// applies it nowhere, because its `parent` is not a file of the theme or its `context` is not
/// a market or B2B.
fn check_contextual(theme: &Theme, directory: &str, name: &str, tally: &mut Tally) {
    let path = format!("{directory}/{name}");
    let Ok(Some(json)) = theme.json(&path) else {
        return;
    };
    let source = theme.files().read(&path).unwrap_or_default();
    let line_of = |key: &str| {
        source
            .lines()
            .position(|line| line.contains(&format!("\"{key}\"")))
            .map(|index| index + 1)
    };
    // The parent its name gives: `index.json` for `index.context.europe.json`.
    let named = name
        .split_once(".context.")
        .map(|(stem, _)| format!("{stem}.json"))
        .unwrap_or_default();
    let parent = json.get("parent").and_then(serde_json::Value::as_str);
    let problem = match parent {
        None => Some((
            "the contextual file has no `parent`: its overrides apply to nothing".to_string(),
            format!("add \"parent\": \"{named}\""),
        )),
        Some(parent) if !theme.files().exists(&format!("{directory}/{parent}")) => Some((
            format!(
                "the parent \"{parent}\" of the contextual file is not in {directory}/: its overrides apply to nothing"
            ),
            format!("name the file it adapts, \"{named}\""),
        )),
        Some(parent) if parent != named => Some((
            format!(
                "the parent \"{parent}\" is not the file the name of the contextual file adapts: its overrides apply to nothing"
            ),
            format!("name it \"{named}\", or rename the file after its parent"),
        )),
        _ => None,
    };
    if let Some((message, hint)) = problem {
        let mut diagnostic = Diagnostic::new(
            "warning",
            "contextual_parent",
            Location::file(&path, line_of("parent")),
            message,
        );
        diagnostic.hint = Some(hint);
        tally.report(diagnostic);
    }
    let context = match lsf_theme::theme::template_json::context_of(&json) {
        None => {
            Some("the contextual file has no `context`: its overrides apply to nobody".to_string())
        }
        Some(Err(why)) => Some(format!("{why}: its overrides apply to nobody")),
        Some(Ok(_)) => None,
    };
    if let Some(message) = context {
        let mut diagnostic = Diagnostic::new(
            "warning",
            "contextual_context",
            Location::file(&path, line_of("context")),
            message,
        );
        diagnostic.hint = Some(
            "Shopify's contexts are {\"market\": \"<market handle>\"} and {\"b2b\": true}"
                .to_string(),
        );
        tally.report(diagnostic);
    }
}

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
    previews: &[lsf_theme::Preview],
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
        let checked = format!(
            "{} checked, {}",
            count(files, "file", "files"),
            count(tally.problems, "problem", "problems")
        );
        if tally.fixed > 0 {
            eprintln!("{checked}, {} fixed", tally.fixed);
        } else {
            eprintln!("{checked}");
        }
        if tally.fixable > 0 {
            eprintln!(
                "{} of them can be fixed: run again with `--fix`",
                tally.fixable
            );
        }
        if tally.changes > 0 {
            eprintln!(
                "{} of the fixes {} the output, where the code now does what it says: review the diff",
                tally.changes,
                if tally.changes == 1 {
                    "changes"
                } else {
                    "change"
                }
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
    previews: &[lsf_theme::Preview],
    args: &Args,
) -> Result<(usize, Tally), String> {
    let env = Arc::new(lsf_theme::environment_with(previews));
    let theme = Theme::open(theme_dir, env.clone(), Revalidate::Never)
        .map_err(|error| error.to_string())?;
    let mut files = 0;
    let mut tally = Tally {
        json: args.format == Format::Json,
        github: (args.format == Format::Github).then(|| theme_dir.to_path_buf()),
        ..Tally::default()
    };
    // A warning, which is not a problem: the theme is fine on Shopify.
    if let Some(note) = crate::app::classic_accounts_note(&theme) {
        tally.report(Diagnostic::new(
            "warning",
            "classic_customer_accounts",
            Location::file("templates/customers", None),
            note,
        ));
    }
    // The keys of the `t` filter are looked up in the default locale.
    let locale = references::default_locale(&theme);
    // The schemas are checked against the theme blocks and the default schema locale.
    let schema_names = references::SchemaNames::of(&theme);
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
            report_encoding(&theme, &path, &mut tally);
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
            for misplaced in placement::problems(directory, &source) {
                tally.problem(
                    "liquid_syntax",
                    Location::file(&path, Some(misplaced.line)),
                    format!("Shopify's parser refuses this file: {}", misplaced.message),
                );
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
            if matches!(directory, "sections" | "blocks") {
                let of = if directory == "blocks" {
                    lsf_theme::theme::schema_check::SchemaOf::Block
                } else {
                    lsf_theme::theme::schema_check::SchemaOf::Section
                };
                for problem in schema_names.problems(&source, of) {
                    tally.problem_with_hint(
                        problem.code,
                        Location::file(&path, Some(problem.line)),
                        problem.message,
                        problem.hint,
                    );
                }
                for lenient in lsf_theme::theme::schema_check::lenient(&source) {
                    tally.report(Diagnostic::new(
                        "warning",
                        lenient.code,
                        Location::file(&path, Some(lenient.line)),
                        lenient.message,
                    ));
                }
            }
            for missing in references::missing_assets(&theme, &source) {
                let mut diagnostic = Diagnostic::new(
                    "warning",
                    missing.code,
                    Location::file(&path, Some(missing.line)),
                    missing.message,
                );
                diagnostic.hint = missing.hint;
                tally.report(diagnostic);
            }
            for missing in references::in_liquid(&theme, &source, locale.as_deref()) {
                if missing.is_warning() {
                    let mut diagnostic = Diagnostic::new(
                        "warning",
                        missing.code,
                        Location::file(&path, Some(missing.line)),
                        missing.message,
                    );
                    diagnostic.hint = missing.hint;
                    tally.report(diagnostic);
                    continue;
                }
                tally.problem_with_hint(
                    missing.code,
                    Location::file(&path, Some(missing.line)),
                    missing.message,
                    missing.hint,
                );
            }
            let mut reported = Vec::new();
            for (filter, line) in filters_used(&source) {
                if env.filter(&filter).is_none() && !reported.contains(&filter) {
                    tally.problem_with_hint(
                        "unknown_filter",
                        Location::file(&path, Some(line)),
                        format!(
                            "unknown filter `{filter}` (Shopify ignores unknown filters; this one is not implemented here)"
                        ),
                        meant_filter(&env, &filter).map(|name| format!("did you mean \"{name}\"?")),
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
            report_encoding(&theme, &path, &mut tally);
            if let Err(error) = theme.json(&path) {
                let message = match error {
                    lsf_core::Error::Json { message, .. } => message,
                    other => other.to_string(),
                };
                tally.problem("invalid_json", Location::file(&path, None), message);
                continue;
            }
            let source = theme.files().read(&path).unwrap_or_default();
            for (line, leniency) in lsf_core::json::theme_json(&source).1 {
                tally.report(Diagnostic::new(
                    "warning",
                    "lenient_json",
                    Location::file(&path, Some(line)),
                    lsf_theme::theme::schema_check::lenient_message("the file", leniency),
                ));
            }
            if holds_settings(&path) {
                check_liquid_settings(&theme, &env, &path, args.writes(), &mut tally);
            }
            if matches!(directory, "templates" | "templates/customers" | "sections")
                && lsf_theme::theme::template_json::is_contextual(name)
            {
                check_contextual(&theme, directory, name, &mut tally);
            }
            if matches!(directory, "templates" | "templates/customers" | "sections") {
                for missing in references::in_json_template(&theme, &path) {
                    tally.problem_with_hint(
                        missing.code,
                        Location::file(&path, Some(missing.line)),
                        missing.message,
                        missing.hint,
                    );
                }
            }
        }
    }
    // The app blocks the templates name: against the theme app extensions of `--app`.
    // With `--app`, an app block that renders nothing is an error: the check is asked for.
    for finding in crate::theme_extensions::check_lines(&theme, args.app.as_deref())? {
        let severity = match finding.level {
            lsf_theme::app_extensions::references::Level::Warning if args.app.is_some() => {
                tally.problems += 1;
                "error"
            }
            lsf_theme::app_extensions::references::Level::Warning => "warning",
            lsf_theme::app_extensions::references::Level::Info => "info",
        };
        let location = Location {
            setting: finding.pointer,
            ..Location::file(&finding.file, None)
        };
        tally.report(Diagnostic::new(
            severity,
            "app_block",
            location,
            finding.message,
        ));
    }
    Ok((files, tally))
}

#[cfg(test)]
mod tests {
    use super::{
        Args, Format, block_call_line, changed_words, check_theme, filters_used, json_report,
        meant_filter, preview_hint, run,
    };

    #[test]
    fn an_unknown_filter_close_to_one_of_shopify_says_which() {
        let env = lsf_theme::environment_with(&[]);
        let meant = |name: &str| meant_filter(&env, name);
        // One letter off, two, the same letters, a name of another language.
        assert_eq!(meant("upcse").as_deref(), Some("upcase"));
        assert_eq!(meant("dwoncase").as_deref(), Some("downcase"));
        assert_eq!(
            meant("money_with_currancy").as_deref(),
            Some("money_with_currency")
        );
        assert_eq!(meant("uppercas").as_deref(), Some("upcase"));
        assert_eq!(meant("uppercase").as_deref(), Some("upcase"));
        assert_eq!(meant("length").as_deref(), Some("size"));
        // Nothing close: no guess.
        assert_eq!(meant("frobnicate"), None);
        assert_eq!(meant("nope"), None);
    }

    #[test]
    fn the_apps_section_shopify_supplies_is_not_missing() {
        // The fixture theme names an `apps` section and has no `sections/apps.liquid`.
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/theme-app-extensions");
        let args = Args {
            fix: false,
            format: Format::Json,
            app: Some(fixture.join("app")),
        };
        let (files, tally) = check_theme(&fixture.join("theme"), &[], &args).unwrap();
        let report = json_report(files, &tally);
        let codes: Vec<&str> = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["code"].as_str().unwrap())
            .collect();
        assert!(!codes.contains(&"missing_section"), "{report}");
    }

    #[test]
    fn what_the_theme_names_and_does_not_have_is_a_problem() {
        let root = std::env::temp_dir().join(format!("lsf-check-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, content: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write(
            "layout/theme.liquid",
            "{% sections 'header-group' %}{% sections 'footr-group' %}{{ content_for_layout }}",
        );
        write(
            "sections/header-group.json",
            r#"{"type": "header", "name": "Header", "sections": {}, "order": []}"#,
        );
        write("sections/main-product.liquid", "{% render 'price' %}");
        write("snippets/price.liquid", "{{ 'products.price' | t }}");
        write(
            "snippets/card.liquid",
            "<p>\n{% render 'prcie' %}\n{{ 'products.foo' | t }}\n{{ 'products.price' | t }}\n{% render 'price' %}",
        );
        write(
            "locales/en.default.json",
            r#"{"products": {"price": "Price"}}"#,
        );
        write(
            "templates/product.json",
            r#"{
  "sections": {
    "main": {
      "type": "main-prodcut"
    }
  },
  "order": ["main"]
}"#,
        );
        let args = Args {
            fix: false,
            format: Format::Json,
            app: None,
        };
        let (files, tally) = check_theme(&root, &[], &args).unwrap();
        let report = json_report(files, &tally);
        let found: Vec<(String, String, u64, String)> = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["code"].as_str().unwrap().to_string(),
                    d["file"].as_str().unwrap().to_string(),
                    d["line"].as_u64().unwrap_or(0),
                    d["hint"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let expected = |code: &str, file: &str, line: u64, hint: &str| {
            (code.to_string(), file.to_string(), line, hint.to_string())
        };
        assert_eq!(
            found,
            [
                expected("missing_section_group", "layout/theme.liquid", 1, ""),
                expected(
                    "missing_snippet",
                    "snippets/card.liquid",
                    2,
                    "did you mean \"price\"?"
                ),
                expected(
                    "missing_translation",
                    "snippets/card.liquid",
                    3,
                    "add it to locales/en.default.json"
                ),
                expected(
                    "missing_section",
                    "templates/product.json",
                    4,
                    "did you mean \"main-product\"?"
                ),
            ],
            "{report:#}"
        );
        assert_eq!(report["problems"], 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_block_type_that_differs_by_case_is_a_warning_not_a_missing_block() {
        // Shopify renders blocks/text.liquid for `type: 'TEXT'` (#197).
        let root = std::env::temp_dir().join(format!("lsf-check-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, content: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write("layout/theme.liquid", "{{ content_for_layout }}");
        write(
            "sections/holder.liquid",
            "{% content_for 'block', type: 'TEXT', id: 't' %}\n{% content_for 'block', type: 'texte', id: 'u' %}\n{% schema %}\n{ \"name\": \"Holder\", \"settings\": [] }\n{% endschema %}",
        );
        write(
            "blocks/text.liquid",
            "<p>text block</p>{% schema %}{ \"name\": \"Text\", \"settings\": [] }{% endschema %}",
        );
        let args = Args {
            fix: false,
            format: Format::Json,
            app: None,
        };
        let (files, tally) = check_theme(&root, &[], &args).unwrap();
        let report = json_report(files, &tally);
        let found: Vec<(&str, &str, u64, &str)> = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["severity"].as_str().unwrap(),
                    d["code"].as_str().unwrap(),
                    d["line"].as_u64().unwrap_or(0),
                    d["hint"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                ("warning", "block_type_case", 1, "write type: 'text'"),
                ("error", "missing_block", 2, "did you mean \"text\"?"),
            ],
            "{report:#}"
        );
        assert_eq!(report["problems"], 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_that_is_not_utf8_is_a_warning_and_is_read_as_shopify_stores_it() {
        // #176: a section, a snippet and a JSON template saved in Latin-1 (`é` is 0xE9).
        let root = std::env::temp_dir().join(format!("lsf-check-latin1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, content: &[u8]| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write(
            "layout/theme.liquid",
            b"<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>\n",
        );
        write(
            "sections/latin.liquid",
            b"<p>section: caf\xe9</p>\n{% schema %}\n{ \"name\": \"Latin\", \"settings\": [] }\n{% endschema %}\n",
        );
        write(
            "sections/uses-snippet.liquid",
            b"<p>{% render 'latin' %}</p>\n{% schema %}\n{ \"name\": \"Uses snippet\", \"settings\": [] }\n{% endschema %}\n",
        );
        write("snippets/latin.liquid", b"<i>snippet: caf\xe9</i>\n");
        write(
            "templates/index.json",
            b"{ \"sections\": { \"a\": { \"type\": \"latin\" }, \"b\": { \"type\": \"uses-snippet\" } }, \"order\": [\"a\", \"b\"] }\n",
        );
        write(
            "templates/list-collections.json",
            b"{ \"sections\": { \"a\": { \"type\": \"uses-snippet\" } },\n\"order\": [\"a\"], \"wrapper\": \"div.caf\xe9\" }\n",
        );
        let args = Args {
            fix: false,
            format: Format::Json,
            app: None,
        };
        let (files, tally) = check_theme(&root, &[], &args).unwrap();
        let report = json_report(files, &tally);
        let found: Vec<(&str, &str, &str, u64)> = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["severity"].as_str().unwrap(),
                    d["code"].as_str().unwrap(),
                    d["file"].as_str().unwrap(),
                    d["line"].as_u64().unwrap_or(0),
                )
            })
            .collect();
        // The section and the snippet are found: nothing is missing.
        assert_eq!(
            found,
            [
                ("warning", "not_utf8", "sections/latin.liquid", 1),
                ("warning", "not_utf8", "snippets/latin.liquid", 1),
                ("warning", "not_utf8", "templates/list-collections.json", 2),
            ],
            "{report:#}"
        );
        assert_eq!(report["problems"], 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tags_where_shopifys_parser_refuses_them_are_problems() {
        // #185: a section group in a section, `content_for 'blocks'` twice, a schema in a
        // snippet.
        let root = std::env::temp_dir().join(format!("lsf-check-placement-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, content: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write(
            "layout/theme.liquid",
            "<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
        );
        write(
            "sections/header-group.json",
            r#"{"type": "header", "name": "Header", "sections": {"header": {"type": "header"}}, "order": ["header"]}"#,
        );
        write(
            "sections/header.liquid",
            "<header>header</header>\n{% schema %}\n{ \"name\": \"Header\", \"settings\": [] }\n{% endschema %}",
        );
        write(
            "sections/group-inside.liquid",
            "<div>{% sections 'header-group' %}</div>\n{% schema %}\n{ \"name\": \"Group inside\", \"settings\": [] }\n{% endschema %}",
        );
        write(
            "sections/blocks-twice.liquid",
            "{% content_for 'blocks' %}{% content_for 'blocks' %}\n{% schema %}\n{ \"name\": \"Blocks twice\", \"blocks\": [ { \"type\": \"@theme\" } ] }\n{% endschema %}",
        );
        write(
            "sections/uses-snippet.liquid",
            "{% render 'with-schema' %}\n{% schema %}\n{ \"name\": \"Uses snippet\", \"settings\": [] }\n{% endschema %}",
        );
        write(
            "snippets/with-schema.liquid",
            "<p>snippet</p>\n{% schema %}\n{ \"name\": \"Snippet\" }\n{% endschema %}",
        );
        write(
            "templates/index.json",
            r#"{"sections": {"a": {"type": "group-inside"}, "b": {"type": "blocks-twice"}, "c": {"type": "uses-snippet"}}, "order": ["a", "b", "c"]}"#,
        );
        let args = Args {
            fix: false,
            format: Format::Json,
            app: None,
        };
        let (files, tally) = check_theme(&root, &[], &args).unwrap();
        let report = json_report(files, &tally);
        let found: Vec<(&str, &str, u64)> = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["code"].as_str().unwrap(),
                    d["file"].as_str().unwrap(),
                    d["line"].as_u64().unwrap_or(0),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                ("liquid_syntax", "sections/blocks-twice.liquid", 1),
                ("liquid_syntax", "sections/group-inside.liquid", 1),
                ("liquid_syntax", "snippets/with-schema.liquid", 2),
            ],
            "{report:#}"
        );
        assert!(
            report["diagnostics"][1]["message"]
                .as_str()
                .unwrap()
                .contains("Unknown tag 'sections'"),
            "{report:#}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn schema_attributes_shopify_refuses_are_problems() {
        // #182: a 26-character name, theme blocks with a block of the section, a theme block
        // with `max_blocks`.
        let root =
            std::env::temp_dir().join(format!("lsf-check-attributes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, content: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write(
            "layout/theme.liquid",
            "{{ content_for_header }}{{ content_for_layout }}",
        );
        write(
            "sections/long-name.liquid",
            "<p>long name</p>\n{% schema %}\n{ \"name\": \"Abcdefghijklmnopqrstuvwxyz\", \"settings\": [] }\n{% endschema %}",
        );
        write(
            "sections/mixed-blocks.liquid",
            "{% content_for \"blocks\" %}\n{% schema %}\n{ \"name\": \"Mixed blocks\", \"blocks\": [ { \"type\": \"@theme\" }, { \"type\": \"local\", \"name\": \"Local\" } ] }\n{% endschema %}",
        );
        write(
            "sections/holder.liquid",
            "{% content_for 'blocks' %}{% schema %}{ \"name\": \"Holder\", \"blocks\": [{ \"type\": \"@theme\" }] }{% endschema %}",
        );
        write(
            "blocks/item.liquid",
            "<p>item</p>\n{% schema %}\n{ \"name\": \"Item\", \"max_blocks\": 3, \"settings\": [] }\n{% endschema %}",
        );
        write(
            "templates/index.json",
            r#"{"sections": {"a": {"type": "long-name"}, "b": {"type": "mixed-blocks"}, "c": {"type": "holder", "blocks": {"i": {"type": "item"}}, "block_order": ["i"]}}, "order": ["a", "b", "c"]}"#,
        );
        let args = Args {
            fix: false,
            format: Format::Json,
            app: None,
        };
        let (files, tally) = check_theme(&root, &[], &args).unwrap();
        let report = json_report(files, &tally);
        let found: Vec<(&str, &str, u64)> = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["code"].as_str().unwrap(),
                    d["file"].as_str().unwrap(),
                    d["line"].as_u64().unwrap_or(0),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                ("invalid_schema", "sections/long-name.liquid", 3),
                ("invalid_schema", "sections/mixed-blocks.liquid", 3),
                ("invalid_schema", "blocks/item.liquid", 3),
            ],
            "{report:#}"
        );
        assert_eq!(report["problems"], 3, "{report:#}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn schemas_block_types_and_assets_are_checked() {
        let root = std::env::temp_dir().join(format!("lsf-check-schema-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, content: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write("layout/theme.liquid", "{{ content_for_layout }}");
        write("assets/base.css", "");
        write("assets/app.js.liquid", "");
        write(
            "snippets/head.liquid",
            "{{ 'base.css' | asset_url }}{{ 'app.js' | asset_url }}\n{{ 'bse.css' | asset_url | stylesheet_tag }}",
        );
        write(
            "blocks/price.liquid",
            "{% content_for 'block', type: 'pirce', id: 'x' %}\n{% schema %}{\"name\": \"Price\"}{% endschema %}",
        );
        write(
            "sections/main.liquid",
            "{% content_for 'blocks' %}\n{% schema %}\n{\n  \"name\": \"t:names.main\",\n  \"blocks\": [{ \"type\": \"@theme\" }],\n  \"settings\": [\n    { \"type\": \"chekbox\", \"id\": \"a\", \"label\": \"A\" }\n  ]\n}\n{% endschema %}",
        );
        write(
            "sections/broken.liquid",
            "<p>\n{% schema %}\n{ \"name\": \"Broken\" \"tag\": \"div\" }\n{% endschema %}",
        );
        write(
            "sections/slides.liquid",
            r#"{% schema %}{"name": "Slides", "blocks": [{"type": "slide", "name": "Slide"}]}{% endschema %}"#,
        );
        write(
            "locales/en.default.schema.json",
            r#"{"names": {"other": "Other"}}"#,
        );
        write(
            "templates/index.json",
            r#"{
  "sections": {
    "main": {
      "type": "main",
      "blocks": {
        "p": {
          "type": "pricee",
          "blocks": { "q": { "type": "price" } }
        }
      }
    },
    "slides": {
      "type": "slides",
      "blocks": { "s": { "type": "slide" }, "t": { "type": "slid" } }
    }
  },
  "order": ["main", "slides"]
}"#,
        );
        let args = Args {
            fix: false,
            format: Format::Json,
            app: None,
        };
        let (files, tally) = check_theme(&root, &[], &args).unwrap();
        let report = json_report(files, &tally);
        let found: Vec<(String, String, String, u64, String)> = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["severity"].as_str().unwrap().to_string(),
                    d["code"].as_str().unwrap().to_string(),
                    d["file"].as_str().unwrap().to_string(),
                    d["line"].as_u64().unwrap_or(0),
                    d["hint"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let expected = |severity: &str, code: &str, file: &str, line: u64, hint: &str| {
            (
                severity.to_string(),
                code.to_string(),
                file.to_string(),
                line,
                hint.to_string(),
            )
        };
        assert_eq!(
            found,
            [
                expected("error", "invalid_schema", "sections/broken.liquid", 3, ""),
                expected(
                    "error",
                    "missing_schema_translation",
                    "sections/main.liquid",
                    4,
                    "add it to locales/en.default.schema.json"
                ),
                expected(
                    "error",
                    "unknown_setting_type",
                    "sections/main.liquid",
                    7,
                    "did you mean \"checkbox\"?"
                ),
                expected(
                    "error",
                    "missing_block",
                    "blocks/price.liquid",
                    1,
                    "did you mean \"price\"?"
                ),
                expected(
                    "warning",
                    "missing_asset",
                    "snippets/head.liquid",
                    2,
                    "did you mean \"base.css\"?"
                ),
                expected(
                    "error",
                    "missing_block",
                    "templates/index.json",
                    7,
                    "did you mean \"price\"?"
                ),
                expected(
                    "error",
                    "missing_block",
                    "templates/index.json",
                    14,
                    "did you mean \"slide\"?"
                ),
            ],
            "{report:#}"
        );
        assert_eq!(report["problems"], 6);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn with_an_app_a_block_that_renders_nothing_is_a_problem() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/theme-app-extensions");
        let report = |app: Option<std::path::PathBuf>| {
            let args = Args {
                fix: false,
                format: Format::Json,
                app,
            };
            let (files, tally) = check_theme(&fixture.join("theme"), &[], &args).unwrap();
            json_report(files, &tally)
        };
        let with_app = report(Some(fixture.join("app")));
        let errors: Vec<&str> = with_app["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["code"] == "app_block" && d["severity"] == "error")
            .map(|d| d["message"].as_str().unwrap())
            .collect();
        assert_eq!(errors.len(), 2, "{with_app:#}");
        assert!(errors[0].contains("no block `reviewz`: did you mean `reviews`?"));
        assert!(errors[1].contains("its `enabled_on` leaves out the footer section group"));
        assert_eq!(with_app["problems"], 2);
        assert_eq!(with_app["ok"], false);
        // Without the app, a note: they are not checked.
        let without = report(None);
        assert_eq!(without["problems"], 0, "{without:#}");
    }

    #[test]
    fn a_github_annotation_carries_the_setting_the_fix_and_the_hint() {
        let mut diagnostic = super::Diagnostic::new(
            "error",
            "unknown_filter",
            super::Location {
                setting: Some("sections.a.settings.code".to_string()),
                ..super::Location::file("templates/index.json", Some(3))
            },
            "unknown filter `upcse`".to_string(),
        );
        diagnostic.hint = Some("did you mean \"upcase\"?".to_string());
        assert_eq!(
            diagnostic
                .annotation(std::path::Path::new("theme"))
                .to_string(),
            "::error file=theme/templates/index.json,line=3,title=lsf check%3A unknown_filter::sections.a.settings.code: unknown filter `upcse`%0Ahint: did you mean \"upcase\"?"
        );
    }

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
    fn trailing_commas_and_unknown_escapes_are_warnings_and_a_bom_an_error() {
        let root = std::env::temp_dir().join(format!("lsf-check-lenient-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, content: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write("layout/theme.liquid", "{{ content_for_layout }}");
        write(
            "sections/comma.liquid",
            "{{ section.settings.t }}\n{% schema %}\n{\n  \"name\": \"Comma\",\n  \"settings\": [\n    \
             { \"type\": \"text\", \"id\": \"t\", \"label\": \"T\", \"default\": \"\\u201cq\\“\" },\n  ],\n}\n{% endschema %}",
        );
        write(
            "templates/index.json",
            "{\n  \"sections\": { \"a\": { \"type\": \"comma\" } },\n  \"order\": [\"a\"],\n}",
        );
        write("locales/en.default.json", "\u{feff}{}");
        let args = Args {
            fix: false,
            format: Format::Json,
            app: None,
        };
        let (files, tally) = check_theme(&root, &[], &args).unwrap();
        let report = json_report(files, &tally);
        let found: Vec<(String, String, String, u64)> = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["severity"].as_str().unwrap().to_string(),
                    d["code"].as_str().unwrap().to_string(),
                    d["file"].as_str().unwrap().to_string(),
                    d["line"].as_u64().unwrap_or(0),
                )
            })
            .collect();
        let of = |severity: &str, code: &str, file: &str, line: u64| {
            (
                severity.to_string(),
                code.to_string(),
                file.to_string(),
                line,
            )
        };
        assert_eq!(
            found,
            [
                // The escape `\“`, then the comma after the setting, then the one after `]`.
                of("warning", "lenient_json", "sections/comma.liquid", 6),
                of("warning", "lenient_json", "sections/comma.liquid", 6),
                of("warning", "lenient_json", "sections/comma.liquid", 7),
                of("warning", "lenient_json", "templates/index.json", 3),
                of("error", "invalid_json", "locales/en.default.json", 0),
            ]
        );
        // Only the byte order mark counts: Shopify takes the other files.
        assert_eq!(report["problems"], 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn contextual_files_are_checked_and_one_that_applies_nowhere_is_a_warning() {
        let root =
            std::env::temp_dir().join(format!("lsf-check-contextual-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, content: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write(
            "layout/theme.liquid",
            "{% sections 'header-group' %}{{ content_for_layout }}",
        );
        write("sections/note.liquid", "{{ section.settings.text }}");
        write(
            "sections/header-group.json",
            r#"{ "type": "header", "name": "Header", "sections": { "a": { "type": "note" } }, "order": ["a"] }"#,
        );
        write(
            "sections/header-group.context.europe.json",
            r#"{ "context": { "market": "europe" }, "parent": "header-group.json", "sections": { "a": { "settings": { "text": "x" } } } }"#,
        );
        write(
            "templates/index.json",
            r#"{ "sections": { "a": { "type": "note" } }, "order": ["a"] }"#,
        );
        // Overrides that add a section of a type the theme lacks: checked as in a template.
        write(
            "templates/index.context.b2b.json",
            r#"{ "context": { "b2b": true }, "parent": "index.json", "sections": { "b": { "type": "gone" } } }"#,
        );
        write(
            "templates/index.context.ca.json",
            "{\n  \"context\": { \"market\": \"ca\" },\n  \"sections\": {}\n}",
        );
        write(
            "templates/product.context.ca.json",
            "{\n  \"context\": { \"country\": \"CA\" },\n  \"parent\": \"product.json\"\n}",
        );
        let args = Args {
            fix: false,
            format: Format::Json,
            app: None,
        };
        let (files, tally) = check_theme(&root, &[], &args).unwrap();
        let report = json_report(files, &tally);
        let found: Vec<(String, String, String, u64)> = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["severity"].as_str().unwrap().to_string(),
                    d["code"].as_str().unwrap().to_string(),
                    d["file"].as_str().unwrap().to_string(),
                    d["line"].as_u64().unwrap_or(0),
                )
            })
            .collect();
        let of = |severity: &str, code: &str, file: &str, line: u64| {
            (
                severity.to_string(),
                code.to_string(),
                file.to_string(),
                line,
            )
        };
        assert_eq!(
            found,
            [
                of(
                    "error",
                    "missing_section",
                    "templates/index.context.b2b.json",
                    1
                ),
                of(
                    "warning",
                    "contextual_parent",
                    "templates/index.context.ca.json",
                    0
                ),
                of(
                    "warning",
                    "contextual_parent",
                    "templates/product.context.ca.json",
                    3
                ),
                of(
                    "warning",
                    "contextual_context",
                    "templates/product.context.ca.json",
                    2
                ),
            ]
        );
        assert_eq!(files, 8);
        assert_eq!(report["problems"], 1);
        let _ = std::fs::remove_dir_all(&root);
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
                app: None,
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
        assert!(find("unknown_filter", 5).get("hint").is_none());
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
            "{{ section.settings.code }}{% schema %}{\"settings\": [{\"type\": \"liquid\", \"id\": \"code\", \"label\": \"Code\"}]}{% endschema %}",
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
                    app: None,
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
