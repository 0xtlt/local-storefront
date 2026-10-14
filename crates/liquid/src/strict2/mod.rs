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

mod meaning;
mod print;

use std::ops::Range;

use crate::environment::Environment;
use crate::error::{Error, Result};
use crate::parser::Parser;
use crate::variable::Variable;

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
    pub issue: Issue,
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
    Some(Issue { kind, message, fix })
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
        let source = "{% render 'price',\n    price: p,\n    varies: a and b,\n    style: true\n%}";
        assert_eq!(
            fixed(source),
            "{% render 'price',\n    price: p,\n    varies: a,\n    style: true\n%}"
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
