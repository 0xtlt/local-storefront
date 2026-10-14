use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use lsf_core::Theme;
use lsf_core::theme::Revalidate;
use lsf_core::theme::liquid_settings::{holds_settings, string_literal, string_span};
use lsf_liquid::strict2::{self, BROKEN_TODAY, Finding, IssueKind};

#[derive(clap::Args)]
pub struct Args {
    /// Rewrites the markup Shopify's strict Liquid parser rejects into markup it accepts and
    /// that renders the same as today.
    #[arg(long)]
    fix: bool,
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

/// What the check found, and what `--fix` wrote.
#[derive(Default)]
struct Tally {
    problems: usize,
    fixable: usize,
    fixed: usize,
}

impl Tally {
    /// Prints what Shopify's strict parser does not read in a piece of Liquid (`source`): a
    /// file, or the value of a setting. `location` says where a finding is; `written`, whether
    /// the fixes were written.
    fn strict2(
        &mut self,
        findings: &[Finding],
        source: &str,
        written: bool,
        location: impl Fn(&Finding) -> String,
    ) {
        for finding in findings {
            let place = location(finding);
            let what = subject(finding);
            let markup = finding.markup(source);
            let problem = match finding.issue.kind {
                IssueKind::Rejected => format!(
                    "Shopify's strict Liquid parser rejects this {what}: {}",
                    finding.issue.message
                ),
                IssueKind::Reinterpreted => {
                    format!("Shopify's strict Liquid parser reads this {what} differently")
                }
            };
            match &finding.issue.fix {
                Ok(fix) if written => {
                    self.fixed += 1;
                    let (was, now) = changed_words(markup, fix);
                    println!("{place}: fixed: {problem}");
                    println!("  was: {was}");
                    println!("  now: {now}");
                }
                Ok(fix) => {
                    self.problems += 1;
                    self.fixable += 1;
                    let (was, now) = changed_words(markup, fix);
                    println!("{place}: {problem}");
                    println!("  was: {was}");
                    println!("  fix: {now}");
                }
                Err(reason) => {
                    self.problems += 1;
                    println!("{place}: {problem}");
                    println!("  was: {}", one_line(markup));
                    println!("  no automatic fix: {reason}");
                }
            }
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
/// and with `fix`, writes the fixed values into the file.
fn check_liquid_settings(
    theme: &Theme,
    env: &lsf_liquid::Environment,
    path: &str,
    fix: bool,
    tally: &mut Tally,
) {
    let Some(source) = theme.files().read(path) else {
        return;
    };
    let mut checked = Vec::new();
    let mut replacements: Vec<(Range<usize>, String)> = Vec::new();
    for setting in theme.liquid_settings(path) {
        let findings = reportable(strict2::audit(env, &setting.value).findings);
        if findings.is_empty() {
            continue;
        }
        let span = string_span(&source, &setting.path);
        let fixed = strict2::apply(&setting.value, &findings);
        if let Some(span) = &span
            && fixed != setting.value
        {
            replacements.push((span.clone(), string_literal(&fixed, &source[span.clone()])));
        }
        checked.push((setting, findings, span));
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
    let written = fix
        && !replacements.is_empty()
        && lsf_core::json::parse_lenient(path, &fixed_source).is_ok()
        && write_if_unchanged(theme.files().resolve(path), &source, &fixed_source);
    for (setting, findings, span) in checked {
        let line = span.map_or(1, |span| source[..span.start].matches('\n').count() + 1);
        let dotted = setting.dotted();
        tally.strict2(&findings, &setting.value, written, |finding| {
            if finding.line > 1 {
                format!("{path}:{line}: {dotted}, line {}", finding.line)
            } else {
                format!("{path}:{line}: {dotted}")
            }
        });
    }
}

pub fn run(
    theme_dir: &Path,
    previews: &[lsf_core::Preview],
    args: Args,
) -> Result<ExitCode, String> {
    let env = Arc::new(lsf_core::environment_with(previews));
    let theme = Theme::open(theme_dir, env.clone(), Revalidate::Never)
        .map_err(|error| error.to_string())?;
    let mut files = 0;
    let mut tally = Tally::default();
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
            let findings = reportable(strict2::audit(&env, &source).findings);
            let fixed_source = strict2::apply(&source, &findings);
            let written = args.fix
                && fixed_source != *source
                && write_if_unchanged(theme.files().resolve(&path), &source, &fixed_source);
            tally.strict2(&findings, &source, written, |finding| {
                format!("{path}:{}", finding.line)
            });
            if let Err(error) = theme.liquid(&path) {
                println!("{path}: {error}{}", preview_hint(&error.to_string()));
                tally.problems += 1;
                continue;
            }
            if env.tag("block").is_some()
                && !matches!(directory, "layout" | "templates" | "templates/customers")
                && let Some(line) = block_call_line(&source)
            {
                println!(
                    "{path}:{line}: the block tag can only be used in layout/ and templates/ files"
                );
                tally.problems += 1;
            }
            let mut reported = Vec::new();
            for (filter, line) in filters_used(&source) {
                if env.filter(&filter).is_none() && !reported.contains(&filter) {
                    println!(
                        "{path}:{line}: unknown filter `{filter}` (Shopify ignores unknown filters; this one is not implemented here)"
                    );
                    reported.push(filter);
                    tally.problems += 1;
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
                println!("{error}");
                tally.problems += 1;
                continue;
            }
            if holds_settings(&path) {
                check_liquid_settings(&theme, &env, &path, args.fix, &mut tally);
            }
        }
    }
    let Tally {
        problems,
        fixable,
        fixed,
    } = tally;
    if fixed > 0 {
        eprintln!("{files} file(s) checked, {problems} problem(s), {fixed} fixed");
    } else {
        eprintln!("{files} file(s) checked, {problems} problem(s)");
    }
    if fixable > 0 {
        eprintln!("{fixable} of them can be fixed: run `lsf check --fix`");
    }
    Ok(if problems == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::{Args, block_call_line, changed_words, filters_used, preview_hint, run};

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
            "{% render 'price',\n  price: p,\n  varies: a and b\n%}",
        );
        write("snippets/price.liquid", "{{ price }}");
        write(
            "sections/custom.liquid",
            "{{ section.settings.code }}{% schema %}{\"settings\": [{\"type\": \"liquid\", \"id\": \"code\"}]}{% endschema %}",
        );
        write(
            "templates/index.json",
            "/* edited by Shopify */\n{\"sections\": {\"a\": {\"type\": \"custom\", \"settings\": {\"code\": \"\\u003cb\\u003e{{ x y }}\"}}}, \"order\": [\"a\"]}",
        );
        let check = |fix: bool| run(&root, &[], Args { fix }).unwrap();
        assert_eq!(check(false), std::process::ExitCode::FAILURE);
        check(true);
        let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
        assert_eq!(
            read("snippets/card.liquid"),
            "{% render 'price',\n  price: p,\n  varies: a\n%}"
        );
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
