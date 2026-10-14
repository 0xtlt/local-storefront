//! Shopify's strict Liquid parser, and how to move a template to it.
//!
//! Shopify is moving every store from the forgiving ("lax") parser to a strict one, which the
//! reference gem calls `error_mode: :strict2`. A theme whose markup the strict parser rejects can
//! no longer be uploaded, and the stores Shopify migrates get their files rewritten so that they
//! keep rendering as before. This module does the same locally:
//!
//! - [`audit`] parses a template with the engine and checks each tag and output against the
//!   rules of the strict parser, ported from the gem's `strict2_parse` methods;
//! - a [`Finding`] carries, when there is one, a rewrite of the markup that the strict parser
//!   accepts and that means what the markup means today. The rewrite is printed from what the
//!   engine parsed, and is kept only when parsing it back, strictly and the usual way, gives the
//!   same thing.
//!
//! Each tag says how it is checked with [`crate::Environment::register_syntax`].

pub mod intent;
mod meaning;
mod print;

use std::collections::HashSet;
use std::ops::Range;

use crate::environment::Environment;
use crate::error::{Error, Result};
use crate::parser::Parser;
use crate::variable::Variable;

pub use intent::Intent;
pub(crate) use meaning::literal_key;
pub use print::{
    Printed, Style, arguments, condition, condition_operand, expr, identifier, is_identifier,
    string, variable,
};

/// Why a finding has no fix when the markup does not parse today either.
pub const BROKEN_TODAY: &str = "the markup is an error today too";

/// How a tag's markup is checked: `None` when Shopify's strict parser reads it as it is read
/// today.
pub type SyntaxCheck = fn(&str, &Style) -> Option<Issue>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueKind {
    /// The strict parser refuses the markup: Shopify no longer accepts the file.
    Rejected,
    /// The strict parser accepts the markup but reads it differently from today.
    Reinterpreted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub kind: IssueKind,
    /// The strict parser's error, worded as the reference gem words it.
    pub message: String,
    /// Markup that the strict parser accepts and that renders what the markup renders today,
    /// or why there is none.
    pub fix: std::result::Result<String, String>,
    /// What the markup was meant to do, when that is clear and differs from today's output.
    pub intent: Option<Intent>,
    /// Variables the fix passes to a snippet only because the lax parser read them out of a
    /// filter's arguments, with the fix without them. See [`drop_unread`].
    pub strays: Option<Strays>,
    /// What renders differently with the fix, when it does not keep today's output: where what
    /// the lax parser reads is an accident no one means (a string read as a variable name), the
    /// fix does what the markup says.
    pub changes: Option<String>,
}

/// `{% render 'image', alt: image.alt | default: shop.name %}` passes `default` to the
/// snippet: the lax parser reads `default: shop.name` as an argument of `render`. The fix keeps
/// it, since it renders the same, unless the snippet does not read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Strays {
    /// The snippet that `render` renders.
    pub snippet: String,
    pub variables: Vec<String>,
    /// The fix without them.
    pub fix: String,
}

/// Leaves the stray variables out of the fixes of `render`, when `reads(snippet, variable)`
/// says the snippet does not read them: the snippet is rendered on its own, so they change
/// nothing. Returns the findings whose fix changed, with the variables left out.
pub fn drop_unread(
    findings: &mut [Finding],
    reads: impl Fn(&str, &str) -> bool,
) -> Vec<(usize, Vec<String>)> {
    let mut dropped = Vec::new();
    for (index, finding) in findings.iter_mut().enumerate() {
        let Some(strays) = &finding.issue.strays else {
            continue;
        };
        if finding.issue.fix.is_ok()
            && !strays
                .variables
                .iter()
                .any(|variable| reads(&strays.snippet, variable))
        {
            finding.issue.fix = Ok(strays.fix.clone());
            dropped.push((index, strays.variables.clone()));
        }
    }
    dropped
}

/// Whether `text` mentions `name` as a whole name, or `self`, through which any variable can be
/// read.
pub fn mentions(text: &str, name: &str) -> bool {
    word_positions(text, name).next().is_some() || word_positions(text, "self").next().is_some()
}

impl Issue {
    /// Makes the fix what the markup was meant to do, for a reading of the lax parser that no
    /// one can mean.
    pub fn with_meant_fix(mut self, intent: Option<Intent>) -> Issue {
        if let Some(intent) = intent.filter(|intent| intent.prelude.is_empty())
            && self.kind == IssueKind::Rejected
            && self.fix.is_ok()
        {
            self.fix = Ok(intent.markup);
            self.changes = Some(intent.change);
        }
        self
    }

    /// Adds what the markup was meant to do, unless that is what the fix does already.
    pub fn with_intent(mut self, intent: Option<Intent>) -> Issue {
        self.intent = intent.filter(|intent| {
            self.kind == IssueKind::Rejected
                && (!intent.prelude.is_empty()
                    || intent.body_condition.is_some()
                    || !intent.block.is_empty()
                    || self.fix.as_ref() != Ok(&intent.markup))
        });
        self
    }
}

/// Where a tag or an output starts, for the tags an intent runs before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenStart {
    /// The offset of its `{%` or `{{`, or of its first character on a line of `{% liquid %}`.
    pub offset: usize,
    pub in_liquid_tag: bool,
    /// Whether it opens with `{%-` or `{{-`.
    pub trims_left: bool,
}

/// A piece of markup Shopify's strict parser does not read as it is read today.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The line the markup starts on.
    pub line: u32,
    /// Where the markup is in the source, without the whitespace around it.
    pub span: Range<usize>,
    /// The tag, or `None` for an output (`{{ }}`).
    pub tag: Option<String>,
    /// Where the tag starts; `None` for a delimiter (`elsif`, `when`), where nothing can be put
    /// before it.
    pub token: Option<TokenStart>,
    /// The body that follows a delimiter (`when`), for an intent that puts it in an `if`.
    pub body: Option<Body>,
    pub issue: Issue,
}

/// Where the body of a delimiter is: between the end of its tag and the start of the next tag
/// of its block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Body {
    /// The offset right after the delimiter's tag.
    pub start: usize,
    /// Whether the delimiter's tag closes with `-%}`.
    pub start_trimmed: bool,
    /// The offset of the next tag of the block (its first character on a line of
    /// `{% liquid %}`), once it is known.
    pub end: Option<usize>,
    /// Whether that tag opens with `{%-`.
    pub end_trims: bool,
    pub in_liquid_tag: bool,
    /// Whether the block has an `else`: in a `case`, an `if` inside a `when` is then not what
    /// `when x and y` meant, since the `else` would not run when `y` is false.
    pub has_else: bool,
}

impl Finding {
    /// The markup as it is written in the source.
    pub fn markup<'s>(&self, source: &'s str) -> &'s str {
        &source[self.span.clone()]
    }
}

/// What [`audit`] found in a template.
pub struct Audit {
    pub findings: Vec<Finding>,
    /// The error the template fails to parse with today, if it does: the findings then stop
    /// where the error is.
    pub error: Option<Error>,
}

/// Parses `source` as the engine does and checks its tags and outputs against Shopify's strict
/// parser.
pub fn audit(env: &Environment, source: &str) -> Audit {
    let mut parser = Parser::new(env, source);
    parser.start_audit();
    let error = parser
        .parse_document()
        .err()
        .map(|error| error.with_line(parser.line_number()));
    Audit {
        findings: parser.take_findings(),
        error,
    }
}

/// `source` with the fix of every finding that has one.
pub fn apply<'f>(source: &str, findings: impl IntoIterator<Item = &'f Finding>) -> String {
    let mut fixes: Vec<(&Range<usize>, &str)> = findings
        .into_iter()
        .filter_map(|finding| Some((&finding.span, finding.issue.fix.as_deref().ok()?)))
        .collect();
    fixes.sort_by_key(|(span, _)| span.start);
    let mut out = String::with_capacity(source.len());
    let mut position = 0;
    for (span, fix) in fixes {
        if span.start < position {
            continue;
        }
        out.push_str(&source[position..span.start]);
        out.push_str(fix);
        position = span.end;
    }
    out.push_str(&source[position..]);
    out
}

/// Checks markup that is read into `M`: by the engine today (`today`, `None` when that fails)
/// and by Shopify's strict parser (`strict`).
///
/// When the strict parser fails, or when `compare` is set and it reads something else, the
/// markup is a finding. Its fix is `print` of what is read today, kept when both parsers read
/// it back the same. `compare` is for the markup Shopify parses with its lax parser today
/// (outputs, `when`): elsewhere it already tries the strict parser first.
pub fn check<M: PartialEq>(
    markup: &str,
    style: &Style,
    today: impl Fn(&str) -> Option<M>,
    strict: impl Fn(&str) -> Result<M>,
    print: impl Fn(&M, &Style) -> Printed,
    compare: bool,
) -> Option<Issue> {
    let strictly = strict(markup);
    let Some(current) = today(markup) else {
        // The template does not parse today either: the strict parser's error is what Shopify
        // reports from now on.
        return strictly.err().map(|error| Issue {
            kind: IssueKind::Rejected,
            message: error.message,
            fix: Err(BROKEN_TODAY.to_string()),
            intent: None,
            strays: None,
            changes: None,
        });
    };
    let (kind, message) = match strictly {
        Ok(parsed) if !compare || parsed == current => return None,
        Ok(_) => (
            IssueKind::Reinterpreted,
            "the strict parser reads this differently".to_string(),
        ),
        Err(error) => (IssueKind::Rejected, error.message),
    };
    let fix = print(&current, style).and_then(|fixed| {
        let reads_the_same = strict(&fixed).is_ok_and(|parsed| parsed == current)
            && today(&fixed).is_some_and(|parsed| parsed == current);
        if reads_the_same {
            Ok(fixed)
        } else {
            Err("no rewrite reads the same in both parsers".to_string())
        }
    });
    Some(Issue {
        kind,
        message,
        fix,
        intent: None,
        strays: None,
        changes: None,
    })
}

/// The check of an output (`{{ }}`) and of `echo`.
pub fn check_output(markup: &str, style: &Style) -> Option<Issue> {
    check(
        markup,
        style,
        |markup| Some(Variable::parse(markup)),
        Variable::strict2_parse,
        variable,
        true,
    )
    .map(|issue| issue.with_meant_fix(intent::output(markup, style)))
}

/// Where `word` appears in `text` as a whole name.
fn word_positions<'t>(text: &'t str, word: &'t str) -> impl Iterator<Item = usize> + 't {
    let is_name = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    text.match_indices(word).filter_map(move |(index, _)| {
        let before = text[..index].chars().next_back();
        let after = text[index + word.len()..].chars().next();
        (!before.is_some_and(is_name) && !after.is_some_and(is_name)).then_some(index)
    })
}

/// `text` with every whole occurrence of `from` replaced by `to`.
fn rename(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut position = 0;
    for index in word_positions(text, from).collect::<Vec<_>>() {
        out.push_str(&text[position..index]);
        out.push_str(to);
        position = index + from.len();
    }
    out.push_str(&text[position..]);
    out
}

/// Whether an intent can be written where its finding is: tags to put before a tag need the tag,
/// and a body to put in an `if` needs its end, and no `else` that should run instead.
pub fn applies(intent: &Intent, finding: &Finding) -> bool {
    if !intent.block.is_empty() {
        return true;
    }
    (intent.prelude.is_empty() || finding.token.is_some())
        && (intent.body_condition.is_none()
            || finding
                .body
                .as_ref()
                .is_some_and(|body| body.end.is_some() && !body.has_else))
}

/// Statements one per line, indented by two spaces inside the blocks they open.
fn nested_lines(statements: &[String]) -> Vec<String> {
    let mut depth = 0usize;
    statements
        .iter()
        .map(|statement| {
            let word = statement.split_whitespace().next().unwrap_or_default();
            if word.starts_with("end") || matches!(word, "else" | "elsif" | "when") {
                depth = depth.saturating_sub(1);
            }
            let line = format!("{}{statement}", "  ".repeat(depth));
            if matches!(
                word,
                "if" | "unless" | "case" | "for" | "else" | "elsif" | "when"
            ) {
                depth += 1;
            }
            line
        })
        .collect()
}

/// `source` with the intent of every finding that has one, and the fix of the others: what
/// `lsf check --fix` writes. The temporary variables of the intents get names the file
/// does not use yet, and the fixes leave out what snippets do not read (see [`drop_unread`]).
/// Fails, with the reason, when the result is not what the strict parser accepts.
pub fn apply_intents(
    env: &Environment,
    source: &str,
    reads: impl Fn(&str, &str) -> bool,
) -> std::result::Result<String, String> {
    let mut before = audit(env, source);
    drop_unread(&mut before.findings, reads);
    let used: HashSet<&str> = source
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .collect();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for finding in &before.findings {
        let intent = finding
            .issue
            .intent
            .as_ref()
            .filter(|intent| applies(intent, finding));
        let Some(intent) = intent else {
            if let Ok(fix) = &finding.issue.fix {
                edits.push((finding.span.clone(), fix.clone()));
            }
            continue;
        };
        let mut prelude = intent.prelude.clone();
        let mut markup = intent.markup.clone();
        for temporary in &intent.temporaries {
            let mut name = temporary.clone();
            let mut counter = 1;
            while used.contains(name.as_str()) {
                counter += 1;
                name = format!("{temporary}_{counter}");
            }
            prelude = prelude
                .iter()
                .map(|statement| rename(statement, temporary, &name))
                .collect();
            markup = rename(&markup, temporary, &name);
        }
        if !intent.block.is_empty() {
            // The same block can be rewritten for several findings: its edits are written once.
            for edit in &intent.block {
                if !edits.contains(edit) {
                    edits.push(edit.clone());
                }
            }
            continue;
        }
        if let (Some(condition), Some(body)) = (&intent.body_condition, &finding.body)
            && let Some(end) = body.end
        {
            // The body goes inside `if`, which takes over the trimming of the tags around it.
            let (open, close, end) = if body.in_liquid_tag {
                // `if` and `endif` on lines of their own, at the indentation of the delimiter.
                let line_start = |offset: usize| source[..offset].rfind('\n').map_or(0, |i| i + 1);
                let indent: String = source[line_start(body.start)..]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                (
                    format!("\n{indent}if {condition}"),
                    format!("{indent}endif\n"),
                    line_start(end),
                )
            } else {
                let after = if body.start_trimmed { "-" } else { "" };
                let before = if body.end_trims { "-" } else { "" };
                (
                    format!("{{% if {condition} {after}%}}"),
                    format!("{{%{before} endif %}}"),
                    end,
                )
            };
            edits.push((finding.span.clone(), markup));
            edits.push((body.start..body.start, open));
            edits.push((end..end, close));
            continue;
        }
        match &finding.token {
            Some(token) if !prelude.is_empty() => {
                // The indentation of the tag's line, when the tag starts it.
                let line_start = source[..token.offset].rfind('\n').map_or(0, |i| i + 1);
                let indent = &source[line_start..token.offset];
                let indent = if indent.chars().all(char::is_whitespace) {
                    indent
                } else {
                    ""
                };
                let line_break = if source[..line_start].ends_with("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                };
                let lines = nested_lines(&prelude);
                let text = if token.in_liquid_tag {
                    // One statement per line, at the indentation of the tag.
                    lines
                        .iter()
                        .map(|line| format!("{line}{line_break}{indent}"))
                        .collect::<String>()
                } else {
                    // On the lines above the tag: `-%}` trims the line break that now comes
                    // before the tag, so the output stays the same, and the first tag trims
                    // what the tag trimmed.
                    let dash = if token.trims_left { "-" } else { "" };
                    if let [statement] = lines.as_slice() {
                        format!("{{%{dash} {statement} -%}}{line_break}{indent}")
                    } else {
                        let body: String = lines
                            .iter()
                            .map(|line| format!("{line_break}{indent}  {line}"))
                            .collect();
                        format!(
                            "{{%{dash} liquid{body}{line_break}{indent}-%}}{line_break}{indent}"
                        )
                    }
                };
                let between = &source[token.offset..finding.span.start];
                edits.push((
                    token.offset..finding.span.end,
                    format!("{text}{between}{markup}"),
                ));
            }
            _ => edits.push((finding.span.clone(), markup)),
        }
    }
    edits.sort_by_key(|(span, _)| span.start);
    let mut out = String::with_capacity(source.len());
    let mut position = 0;
    for (span, text) in edits {
        if span.start < position {
            continue;
        }
        out.push_str(&source[position..span.start]);
        out.push_str(&text);
        position = span.end;
    }
    out.push_str(&source[position..]);
    let after = audit(env, &out);
    if after.error.is_some() && before.error.is_none() {
        return Err("the rewritten file does not parse".to_string());
    }
    if after
        .findings
        .iter()
        .any(|finding| finding.issue.fix.is_ok())
    {
        return Err("the strict parser still rejects the rewritten file".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(source: &str) -> Vec<Finding> {
        audit(&Environment::standard(), source).findings
    }

    fn fixed(source: &str) -> String {
        apply(source, &findings(source))
    }

    #[test]
    fn valid_markup_has_no_finding() {
        assert!(
            findings(
                "{{ a.b | f: 1, k: 'v' }}{% if a and b %}{% endif %}{% for x in (1..3) limit: 2 %}{% endfor %}"
            )
            .is_empty()
        );
    }

    #[test]
    fn a_filter_in_a_render_argument_is_dropped_as_the_lax_parser_does() {
        let source = "{% render 'image', alt: image.alt | default: shop.name %}";
        let found = findings(source);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].issue.kind, IssueKind::Rejected);
        assert_eq!(
            found[0].issue.message,
            "Expected end_of_string but found pipe"
        );
        assert_eq!(found[0].tag.as_deref(), Some("render"));
        assert_eq!(
            fixed(source),
            "{% render 'image', alt: image.alt, default: shop.name %}"
        );
    }

    #[test]
    fn spans_leave_the_whitespace_alone() {
        let source = "<p>{{- 'a' 'b' -}}</p>";
        let found = findings(source);
        assert_eq!(found[0].markup(source), "'a' 'b'");
        assert_eq!(fixed(source), "<p>{{- 'a' -}}</p>");
    }

    #[test]
    fn multi_line_arguments_keep_their_layout() {
        let source = "{% render 'card',\n    price: p,\n    on_sale: a and b,\n    style: true\n%}";
        assert_eq!(
            fixed(source),
            "{% render 'card',\n    price: p,\n    on_sale: a,\n    style: true\n%}"
        );
    }

    #[test]
    fn markup_inside_a_liquid_tag_stays_on_its_line() {
        let source = "{% liquid\n  assign a = b c\n  echo a\n%}";
        assert_eq!(fixed(source), "{% liquid\n  assign a = b\n  echo a\n%}");
    }

    /// Each tag: markup the strict parser rejects, its message, and the fix.
    #[test]
    fn fixes_each_tag() {
        let cases = [
            (
                "{{ a b }}",
                "Expected end_of_string but found id",
                "{{ a }}",
            ),
            (
                "{{ ['a'] }}",
                "Bare bracket access is not allowed. Use self['...'] instead",
                "{{ a }}",
            ),
            (
                "{{ [k].b }}",
                "Bare bracket access is not allowed. Use self['...'] instead",
                "{{ self[k].b }}",
            ),
            (
                "{{ x | f, 1 }}",
                "Expected end_of_string but found comma",
                "{{ x | f: 1 }}",
            ),
            // The lax parser reads `('X'` as the variable `X`: no one means that.
            (
                "{% assign a = ('X' | downcase) %}",
                "Expected dotdot but found pipe",
                "{% assign a = 'X' | downcase %}",
            ),
            (
                "{{ (a | upcase) }}",
                "Expected dotdot but found pipe",
                "{{ a | upcase }}",
            ),
            (
                "{% echo a b %}",
                "Expected end_of_string but found id",
                "{% echo a %}",
            ),
            (
                "{% assign a = b c %}",
                "Expected end_of_string but found id",
                "{% assign a = b %}",
            ),
            ("{% assign a b = 1 %}{{ b }}", "", ""),
            (
                "{% capture a b %}{% endcapture %}",
                "Expected end_of_string but found id",
                "{% capture a %}{% endcapture %}",
            ),
            (
                "{% increment a b %}",
                "Expected end_of_string but found id",
                "",
            ),
            // `=` is an operator to the lax parser, which fails when it renders.
            ("{% if a = b %}{% endif %}", "Unexpected character =", ""),
            // `c` is an operator to the lax parser.
            (
                "{% if a %}{% elsif b c %}{% endif %}",
                "Expected end_of_string but found id",
                "",
            ),
            (
                "{% if a %}{% elsif b, %}{% endif %}",
                "Expected end_of_string but found comma",
                "{% if a %}{% elsif b %}{% endif %}",
            ),
            (
                "{% unless a, %}{% endunless %}",
                "Expected end_of_string but found comma",
                "{% unless a %}{% endunless %}",
            ),
            (
                "{% if a == b c %}{% endif %}",
                "Expected end_of_string but found id",
                "{% if a == b %}{% endif %}",
            ),
            (
                "{% case a b %}{% endcase %}",
                "Expected end_of_string but found id",
                "{% case a %}{% endcase %}",
            ),
            (
                "{% case a %}{% when 1 and 2 %}{% endcase %}",
                "Expected end_of_string but found id",
                "{% case a %}{% when 1 %}{% endcase %}",
            ),
            (
                "{% case a %}{% when 1 or 2 3 %}{% endcase %}",
                "Expected end_of_string but found number",
                "{% case a %}{% when 1 or 2 %}{% endcase %}",
            ),
            (
                "{% for x in y z %}{% endfor %}",
                "Invalid attribute in for loop. Valid attributes are limit and offset",
                "{% for x in y %}{% endfor %}",
            ),
            (
                "{% for x in y limit:2 offset:continue %}{% endfor %}",
                "",
                "",
            ),
            (
                "{% tablerow x in y cols:2 rows:3 %}{% endtablerow %}",
                "Invalid attribute 'rows' in tablerow loop. Valid attributes are cols, limit, offset, and range",
                "{% tablerow x in y cols: 2 %}{% endtablerow %}",
            ),
            (
                "{% cycle 'a' 'b', 'c' %}",
                "Expected end_of_string but found string",
                "{% cycle 'a', 'c' %}",
            ),
            ("{% cycle g: 'a', 'b' %}", "", ""),
            ("{% render 'a' b: 1 %}", "", ""),
            (
                "{% render 'a' b %}",
                "Expected colon but found end_of_string",
                "{% render 'a' %}",
            ),
            (
                "{% render 'a', b: c | d %}",
                "Expected end_of_string but found pipe",
                "{% render 'a', b: c %}",
            ),
            ("{% render 'a' with b as c, d: e %}", "", ""),
            (
                "{% include 'a' b c: 1 %}",
                "Expected colon but found id",
                "{% include 'a', c: 1 %}",
            ),
        ];
        for (source, message, fix) in cases {
            let found = findings(source);
            if message.is_empty() {
                assert!(found.is_empty(), "{source}: {found:?}");
                continue;
            }
            assert_eq!(found.len(), 1, "{source}: {found:?}");
            assert_eq!(found[0].issue.message, message, "{source}");
            match fix {
                "" => assert!(found[0].issue.fix.is_err(), "{source}: {found:?}"),
                fix => assert_eq!(fixed(source), fix, "{source}"),
            }
        }
    }

    fn meant(source: &str) -> String {
        apply_intents(&Environment::standard(), source, |_, _| true).unwrap()
    }

    #[test]
    fn a_snippet_that_does_not_read_a_stray_variable_does_not_get_it() {
        let source = "{% render 'image',\n  alt: image.alt | default: shop.name %}";
        let mut found = findings(source);
        assert_eq!(
            drop_unread(&mut found, |snippet, variable| {
                assert_eq!((snippet, variable), ("image", "default"));
                true
            }),
            Vec::new()
        );
        assert_eq!(
            apply(source, &found),
            "{% render 'image',\n  alt: image.alt,\n  default: shop.name %}"
        );
        assert_eq!(
            drop_unread(&mut found, |_, _| false),
            vec![(0, vec!["default".to_string()])]
        );
        assert_eq!(
            apply(source, &found),
            "{% render 'image',\n  alt: image.alt %}"
        );
        // An argument also given outside the filter is not a stray.
        let mut found = findings("{% render 'a', default: 1, b: c | default: 2 %}");
        assert!(found[0].issue.strays.is_none());
        assert!(drop_unread(&mut found, |_, _| false).is_empty());
        assert!(mentions("{{ default }}", "default"));
        assert!(mentions("{{ self[x] }}", "default"));
        assert!(!mentions("{{ a | default: 1 }}x", "defaults"));
    }

    #[test]
    fn intents_do_what_the_markup_meant() {
        assert_eq!(
            meant("<p>\n  {%- render 'image', alt: image.alt | default: shop.name -%}"),
            "<p>\n  {%- assign image_alt = image.alt | default: shop.name -%}\n  {%- render 'image', alt: image_alt -%}"
        );
        assert_eq!(
            meant("{% render 'price',\n  price: p,\n  on_sale: a and b\n%}"),
            "{% liquid\n  assign price_on_sale = false\n  if a and b\n    assign price_on_sale = true\n  endif\n-%}\n{% render 'price',\n  price: p,\n  on_sale: price_on_sale\n%}"
        );
        assert_eq!(
            meant("{% liquid\n  if x\n    render 'image', alt: a | upcase\n  endif\n%}"),
            "{% liquid\n  if x\n    assign image_alt = a | upcase\n    render 'image', alt: image_alt\n  endif\n%}"
        );
        // A name the file uses is not taken.
        assert_eq!(
            meant("{{ image_alt }}{% render 'image', alt: a | upcase %}"),
            "{{ image_alt }}{% assign image_alt_2 = a | upcase -%}\n{% render 'image', alt: image_alt_2 %}"
        );
        assert_eq!(
            meant("{% assign a = ('X' | downcase) %}"),
            "{% assign a = 'X' | downcase %}"
        );
        assert_eq!(
            meant("{% for i in x limit: 2 reversed %}{% endfor %}"),
            "{% for i in x reversed limit: 2 %}{% endfor %}"
        );
        assert_eq!(
            meant("{% if a = b %}{% endif %}"),
            "{% if a == b %}{% endif %}"
        );
        // The body of `when x and y` goes in `if y`, with the trimming of the tags around it.
        assert_eq!(
            meant(
                "{% case t %}\n  {%- when 'c' and h and k -%}\n    x\n  {%- when 'd' %}y{% endcase %}"
            ),
            "{% case t %}\n  {%- when 'c' -%}{% if h and k -%}\n    x\n  {%- endif %}{%- when 'd' %}y{% endcase %}"
        );
        assert_eq!(
            meant("{% liquid\n  case t\n    when 'c' and h\n      echo x\n  endcase\n%}"),
            "{% liquid\n  case t\n    when 'c'\n    if h\n      echo x\n    endif\n  endcase\n%}"
        );
        // With an `else`, which `when 'c' and h` meant to run when `h` is false: `if`/`elsif`.
        assert_eq!(
            meant(
                "<ul>\n  {%- case t -%}\n    {%- when 'p', 'q' -%}<li>p</li>\n    {%- when 'c' and h -%}<li>c</li>\n    {%- else -%}<li>z</li>\n  {%- endcase -%}\n</ul>"
            ),
            "<ul>\n  {%- if t == 'p' or t == 'q' -%}<li>p</li>\n    {%- elsif t == 'c' and h -%}<li>c</li>\n    {%- else -%}<li>z</li>\n  {%- endif -%}\n</ul>"
        );
        assert_eq!(
            meant(
                "{% liquid\n  case t\n    when 'c' and h\n      echo 1\n    else\n      echo 2\n  endcase\n%}"
            ),
            "{% liquid\n  if t == 'c' and h\n      echo 1\n    else\n      echo 2\n  endif\n%}"
        );
        // Values that are not distinct literals: several `when`s could render, so the fix.
        assert_eq!(
            meant("{% case t %}{% when a and h %}x{% else %}y{% endcase %}"),
            "{% case t %}{% when a %}x{% else %}y{% endcase %}"
        );
        assert_eq!(
            meant("{% case t %}{% when 1 %}a{% when 1.0 and h %}x{% else %}y{% endcase %}"),
            "{% case t %}{% when 1 %}a{% when 1.0 %}x{% else %}y{% endcase %}"
        );
        // Without an intent, the fix.
        assert_eq!(meant("{{ a b }}"), "{{ a }}");
        // An `elsif` cannot have tags put before it.
        assert_eq!(
            meant("{% if a %}{% elsif b = c %}{% endif %}"),
            "{% if a %}{% elsif b == c %}{% endif %}"
        );
    }

    #[test]
    fn findings_stop_at_a_parse_error() {
        let result = audit(&Environment::standard(), "{{ a b }}{% if %}{{ c d }}");
        assert_eq!(result.findings.len(), 2);
        assert_eq!(
            result.findings[1].issue.message,
            "[:end_of_string] is not a valid expression"
        );
        assert_eq!(result.findings[1].issue.fix, Err(BROKEN_TODAY.to_string()));
        assert!(result.error.is_some());
    }
}
