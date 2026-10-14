//! JavaScript, formatted by Oxc's formatter, a port of Prettier's printer of JavaScript. It
//! prints a script from the indentation the script is printed at, as Prettier prints the
//! document of an embedded script; what it prints is then laid out as that document is: its
//! lines indented with the script, and what is inside a template literal left where it is.

use oxc::allocator::{Allocator, ArenaVec};
use oxc::ast::ast::{StringLiteral, TemplateElement};
use oxc::ast_visit::Visit;
use oxc::parser::{ParseOptions, Parser};
use oxc::span::SourceType;
use oxc_formatter::{
    ArrowParentheses, AttributePosition, BracketSameLine, BracketSpacing, Expand, JsFormatOptions,
    OperatorPosition, QuoteProperties, QuoteStyle, Semicolons, TrailingCommas,
};
use oxc_formatter_core::{
    Document, FormatContext, FormatElement, FormatOptions, IndentStyle, IndentWidth, LineMode,
    LineWidth,
};

use crate::ast::{is_blank, is_space, trim_end};
use crate::doc::{Doc, at_indent, concat, hardline, literalline, text};
use crate::options::{self, HtmlWhitespaceSensitivity, Options};

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

    // A line continuation.
    fn visit_string_literal(&mut self, literal: &StringLiteral<'a>) {
        self.ranges
            .push((literal.span.start as usize, literal.span.end as usize));
    }
}

/// Prettier indents a block comment again when each of its lines starts with a `*`.
fn is_indentable_block_comment(comment: &str) -> bool {
    comment
        .split('\n')
        .skip(1)
        .all(|line| line.trim_start_matches(is_space).starts_with('*'))
}

/// The source types a script is parsed as, in turn: as a module first, then as a script, as
/// Prettier does.
const SOURCE_TYPES: [fn() -> SourceType; 2] = [SourceType::mjs, SourceType::cjs];

/// The ranges of a formatted script to leave as they are.
fn verbatim_ranges(source: &str, source_type: SourceType) -> Vec<(usize, usize)> {
    let allocator = Allocator::default();
    let options = ParseOptions {
        allow_return_outside_function: true,
        ..ParseOptions::default()
    };
    let parsed = Parser::new(&allocator, source, source_type.with_jsx(true))
        .with_options(options)
        .parse();
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
    verbatim.ranges
}

/// Prettier's options, as Oxc's formatter takes them, for a script laid out in `width` columns.
fn js_options(options: &Options, width: usize) -> JsFormatOptions {
    let quote = |single| {
        if single {
            QuoteStyle::Single
        } else {
            QuoteStyle::Double
        }
    };
    JsFormatOptions {
        indent_style: if options.use_tabs {
            IndentStyle::Tab
        } else {
            IndentStyle::Space
        },
        indent_width: u8::try_from(options.tab_width)
            .ok()
            .and_then(|width| IndentWidth::try_from(width.min(IndentWidth::MAX)).ok())
            .unwrap_or_default(),
        line_width: LineWidth::try_from(
            width.clamp(LineWidth::MIN as usize, LineWidth::MAX as usize) as u16,
        )
        .unwrap_or_default(),
        // Shopify's plugin gives the script `embeddedSingleQuote` as `singleQuote`.
        quote_style: quote(options.embedded_single_quote),
        jsx_quote_style: quote(options.jsx_single_quote),
        quote_properties: match options.quote_props {
            options::QuoteProps::AsNeeded => QuoteProperties::AsNeeded,
            options::QuoteProps::Consistent => QuoteProperties::Consistent,
            options::QuoteProps::Preserve => QuoteProperties::Preserve,
        },
        trailing_commas: match options.trailing_comma {
            options::TrailingComma::All => TrailingCommas::All,
            options::TrailingComma::Es5 => TrailingCommas::Es5,
            options::TrailingComma::None => TrailingCommas::None,
        },
        semicolons: if options.semi {
            Semicolons::Always
        } else {
            Semicolons::AsNeeded
        },
        arrow_parentheses: if options.arrow_parens {
            ArrowParentheses::Always
        } else {
            ArrowParentheses::AsNeeded
        },
        bracket_spacing: BracketSpacing::from(options.bracket_spacing),
        bracket_same_line: BracketSameLine::from(options.bracket_same_line),
        attribute_position: if options.single_attribute_per_line {
            AttributePosition::Multiline
        } else {
            AttributePosition::Auto
        },
        expand: match options.object_wrap {
            options::ObjectWrap::Preserve => Expand::Auto,
            options::ObjectWrap::Collapse => Expand::Never,
        },
        operator_position: if options.operator_at_start {
            OperatorPosition::Start
        } else {
            OperatorPosition::End
        },
        html_whitespace_sensitivity_ignore: options.html_whitespace_sensitivity
            == HtmlWhitespaceSensitivity::Ignore,
        ..JsFormatOptions::default()
    }
}

/// A line of a formatted script, and whether it starts inside a range to leave as it is.
type Line = (String, bool);

/// The lines of `source` formatted from an indentation of `indentation` columns, without that
/// indentation, or `None` when it does not parse as `source_type`.
fn format_at(
    source: &str,
    source_type: SourceType,
    options: &Options,
    indentation: usize,
) -> Option<Vec<Line>> {
    // Prettier measures some things against the whole width, such as a short argument: the
    // script is printed from a level of indentation, in the whole width. An indentation that
    // is not a number of levels is taken from the width.
    let tab_width = options.tab_width.max(1);
    let (level, width) = if indentation.is_multiple_of(tab_width) {
        (indentation / tab_width, options.print_width)
    } else {
        (0, options.print_width.saturating_sub(indentation))
    };
    let allocator = Allocator::default();
    let formatted =
        oxc_formatter::format(&allocator, source, source_type, js_options(options, width)).ok()?;
    // Oxc's printer neither indents nor measures the line it starts on: the script starts
    // after a line break, as it does in Prettier's document.
    let print_options = formatted.context().options().as_print_options();
    let (elements, tailwind_classes) = formatted
        .document()
        .clone()
        .into_elements_and_tailwind_classes();
    let mut after_line_break = ArenaVec::with_capacity_in(elements.len() + 1, &&allocator);
    after_line_break.push(FormatElement::Line(LineMode::Hard));
    after_line_break.extend(elements.iter().cloned());
    let printed = Document::new(after_line_break, tailwind_classes)
        .print_with_indent(source.len(), print_options, u16::try_from(level).ok()?)
        .ok()?
        .into_code();
    let printed = printed.strip_prefix('\n').unwrap_or(&printed);
    let ranges = verbatim_ranges(printed, source_type);
    let prefix = if options.use_tabs {
        "\t".repeat(level)
    } else {
        " ".repeat(level * tab_width)
    };
    let is_verbatim = |offset: usize| {
        ranges
            .iter()
            .any(|(start, end)| *start < offset && offset <= *end)
    };
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in printed.split('\n') {
        let verbatim = is_verbatim(offset);
        let kept = if verbatim {
            line
        } else {
            line.strip_prefix(prefix.as_str()).unwrap_or(line)
        };
        lines.push((kept.to_string(), verbatim));
        offset += line.len() + 1;
    }
    Some(lines)
}

/// The document of a script, or `None` when it does not parse: it is then printed as any
/// text is.
pub(crate) fn format(source: &str, options: &Options) -> Option<Doc> {
    // What the script is parsed as, and how it is formatted at the root, which says whether
    // it parses.
    let (source_type, at_root) = SOURCE_TYPES.iter().find_map(|source_type| {
        let source_type = source_type();
        format_at(source, source_type, options, 0).map(|formatted| (source_type, formatted))
    })?;
    let source = source.to_string();
    let options = options.clone();
    Some(at_indent(move |indentation| {
        let formatted = if indentation == 0 {
            None
        } else {
            format_at(&source, source_type, &options, indentation)
        };
        lay_out(formatted.as_ref().unwrap_or(&at_root))
    }))
}

/// The document of a formatted script: its lines, indented with it, but those that start
/// inside a range to leave as it is.
fn lay_out(lines: &[Line]) -> Doc {
    // A script starts at its first line that holds something, and ends at its last.
    let (Some(first), Some(last)) = (
        lines.iter().position(|(line, _)| !is_blank(line)),
        lines.iter().rposition(|(line, _)| !is_blank(line)),
    ) else {
        return text("");
    };
    let lines = &lines[first..=last];

    let mut parts = Vec::new();
    for (index, (line, verbatim)) in lines.iter().enumerate() {
        // The whitespace a line ends with stays when the line ends inside a range.
        let ends_inside = lines.get(index + 1).is_some_and(|(_, verbatim)| *verbatim);
        let line = if ends_inside {
            line.as_str()
        } else {
            trim_end(line)
        };
        if *verbatim {
            parts.push(literalline());
        } else if index > 0 {
            parts.push(hardline());
        }
        parts.push(text(line));
    }
    concat(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{PrintOptions, indent, print};

    fn fmt_with(source: &str, options: &Options) -> Option<String> {
        let body = format(source, options)?;
        Some(print(
            indent(vec![hardline(), body]),
            &PrintOptions {
                print_width: options.print_width,
                tab_width: options.tab_width,
                use_tabs: options.use_tabs,
            },
        ))
    }

    fn fmt(source: &str) -> Option<String> {
        fmt_with(
            source,
            &Options {
                print_width: 40,
                ..Options::default()
            },
        )
    }

    #[test]
    fn formats_a_script_in_the_columns_its_indentation_leaves() {
        assert_eq!(
            fmt("\n      if (a) {\n   b(\"one\", two, three, four, five)\n }\n    ").unwrap(),
            "\n  if (a) {\n    b('one', two, three, four, five);\n  }"
        );
        // 39 columns, 41 once indented: too wide.
        assert_eq!(
            fmt("b('one', two, three, four, five, sixs);").unwrap(),
            "\n  b(\n    'one',\n    two,\n    three,\n    four,\n    five,\n    sixs,\n  );"
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
    fn follows_the_options_of_prettier_for_javascript() {
        let options = Options {
            semi: false,
            embedded_single_quote: false,
            trailing_comma: options::TrailingComma::None,
            arrow_parens: false,
            ..Options::default()
        };
        assert_eq!(
            fmt_with("const f = (a) => ({ b: 'c' });", &options).unwrap(),
            "\n  const f = a => ({ b: \"c\" })"
        );
    }

    #[test]
    fn what_does_not_parse_is_not_a_script() {
        assert!(fmt("var a = {{ product | json }};").is_none());
        assert!(fmt("<div>").is_none());
    }
}
