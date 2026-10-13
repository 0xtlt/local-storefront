//! JavaScript. It is not reformatted: this crate holds no port of Prettier's JavaScript
//! printer. A script that parses is laid out as Prettier lays out a script it has formatted,
//! so that one Prettier has formatted comes out unchanged: its lines keep their indentation
//! relative to each other, and what is inside a template literal is left where it is.

use oxc::allocator::Allocator;
use oxc::ast::ast::TemplateElement;
use oxc::ast_visit::Visit;
use oxc::parser::{ParseOptions, Parser};
use oxc::span::SourceType;

use crate::ast::{is_blank, is_space, trim_end};
use crate::doc::{Doc, concat, hardline, literalline, text};

/// The ranges of a script whose line breaks are part of what the script means, or of what a
/// comment looks like.
struct Verbatim {
    ranges: Vec<(usize, usize)>,
}

impl<'a> Visit<'a> for Verbatim {
    fn visit_template_element(&mut self, element: &TemplateElement<'a>) {
        self.ranges
            .push((element.span.start as usize, element.span.end as usize));
    }
}

/// Prettier indents a block comment again when each of its lines starts with a `*`.
fn is_indentable_block_comment(comment: &str) -> bool {
    comment
        .split('\n')
        .skip(1)
        .all(|line| line.trim_start_matches(is_space).starts_with('*'))
}

/// The ranges to leave as they are, or `None` when the source is not a script that parses.
fn verbatim_ranges(source: &str) -> Option<Vec<(usize, usize)>> {
    // As a module first, then as a script, as Prettier does.
    for source_type in [
        SourceType::mjs().with_jsx(true),
        SourceType::cjs().with_jsx(true),
    ] {
        let allocator = Allocator::default();
        let options = ParseOptions {
            allow_return_outside_function: true,
            ..ParseOptions::default()
        };
        let parsed = Parser::new(&allocator, source, source_type)
            .with_options(options)
            .parse();
        if parsed.panicked || !parsed.errors.is_empty() {
            continue;
        }
        let mut verbatim = Verbatim { ranges: Vec::new() };
        verbatim.visit_program(&parsed.program);
        for comment in &parsed.program.comments {
            let (start, end) = (comment.span.start as usize, comment.span.end as usize);
            let comment = &source[start..end];
            if comment.starts_with("/*")
                && comment.contains('\n')
                && !is_indentable_block_comment(comment)
            {
                verbatim.ranges.push((start, end));
            }
        }
        return Some(verbatim.ranges);
    }
    None
}

/// The document of a script, or `None` when it does not parse: it is then printed as any
/// text is.
pub(crate) fn format(source: &str) -> Option<Doc> {
    let ranges = verbatim_ranges(source)?;
    let is_verbatim = |offset: usize| {
        ranges
            .iter()
            .any(|(start, end)| *start < offset && offset <= *end)
    };

    // (the line, whether it starts inside a range to leave as it is)
    let mut lines: Vec<(&str, bool)> = Vec::new();
    let mut offset = 0;
    for line in source.split('\n') {
        lines.push((line, is_verbatim(offset)));
        offset += line.len() + 1;
    }
    // A script starts at its first line that holds something, and ends at its last.
    let first = lines.iter().position(|(line, _)| !is_blank(line))?;
    let last = lines.iter().rposition(|(line, _)| !is_blank(line))?;
    let lines = &lines[first..=last];

    let indentation = |line: &str| line.chars().take_while(|c| is_space(*c)).count();
    let min_indent = lines
        .iter()
        .filter(|(line, verbatim)| !verbatim && !is_blank(line))
        .map(|(line, _)| indentation(line))
        .min()
        .unwrap_or(0);

    let mut parts = Vec::new();
    for (index, (line, verbatim)) in lines.iter().enumerate() {
        // The whitespace a line ends with stays when the line ends inside a range.
        let ends_inside = lines.get(index + 1).is_some_and(|(_, verbatim)| *verbatim);
        let line = if ends_inside { line } else { trim_end(line) };
        if *verbatim {
            parts.push(literalline());
            parts.push(text(line));
            continue;
        }
        if index > 0 {
            parts.push(hardline());
        }
        let stripped = line
            .char_indices()
            .nth(min_indent)
            .map_or("", |(offset, _)| &line[offset..]);
        parts.push(text(if indentation(line) >= min_indent {
            stripped
        } else {
            line
        }));
    }
    Some(concat(parts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{PrintOptions, indent, print};

    fn fmt(source: &str) -> Option<String> {
        let body = format(source)?;
        Some(print(
            indent(vec![hardline(), body]),
            &PrintOptions {
                print_width: 80,
                tab_width: 2,
                use_tabs: false,
            },
        ))
    }

    #[test]
    fn lines_keep_their_relative_indentation() {
        assert_eq!(
            fmt("\n      if (a) {\n        b();\n      }\n    ").unwrap(),
            "\n  if (a) {\n    b();\n  }"
        );
    }

    #[test]
    fn what_a_template_literal_holds_is_left_where_it_is() {
        assert_eq!(
            fmt("    const a = `one\n  two\n      three`;\n    b();").unwrap(),
            "\n  const a = `one\n  two\n      three`;\n  b();"
        );
    }

    #[test]
    fn a_documentation_comment_is_indented_and_another_is_not() {
        assert_eq!(
            fmt("    /**\n     * a\n     */\n    b();").unwrap(),
            "\n  /**\n   * a\n   */\n  b();"
        );
        assert_eq!(
            fmt("    /* a\n  b */\n    c();").unwrap(),
            "\n  /* a\n  b */\n  c();"
        );
    }

    #[test]
    fn what_does_not_parse_is_not_a_script() {
        assert!(fmt("var a = {{ product | json }};").is_none());
        assert!(fmt("<div>").is_none());
    }
}
