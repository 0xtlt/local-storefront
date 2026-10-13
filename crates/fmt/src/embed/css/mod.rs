//! CSS, as Prettier formats it: a port of its PostCSS printer, for plain CSS.
//!
//! The style sheet is cut the way PostCSS cuts it ([`parser`]), and each part is then read by
//! a port of the parser Prettier gives it to: values ([`value`]), selectors ([`selector`]) and
//! media queries ([`media`]).

mod media;
mod misc;
mod parser;
mod selector;
mod value;

use crate::doc::{Doc, concat, group, hardline, indent, softline, text};
use crate::options::Options;

use misc::{is_next_line_empty, maybe_to_lower_case};
use parser::{Kind, Node, Range};

struct Printer<'a> {
    source: &'a str,
    single_quote: bool,
}

/// The at-rules whose parameters are read as a value.
const VALUE_AT_RULES: [&str; 14] = [
    "namespace",
    "supports",
    "if",
    "else",
    "for",
    "each",
    "while",
    "debug",
    "mixin",
    "include",
    "function",
    "return",
    "define-mixin",
    "add-mixin",
];

impl Printer<'_> {
    fn slice(&self, range: Range) -> &str {
        &self.source[range.start..range.end]
    }

    /// The nodes of a block, one per line, with the blank lines that were between them.
    fn print_sequence(&self, nodes: &[Node], at_rule: Option<&str>) -> Doc {
        let mut parts = Vec::new();
        for (index, node) in nodes.iter().enumerate() {
            let ignored = index > 0
                && matches!(nodes[index - 1].kind, Kind::Comment)
                && comment_text(self.slice(nodes[index - 1].range)) == "prettier-ignore";
            parts.push(if ignored {
                text(self.slice(node.range))
            } else {
                self.print_node(node, at_rule)
            });
            let Some(next) = nodes.get(index + 1) else {
                break;
            };
            // A comment that follows a node on its line stays on that line.
            let on_same_line = !self.source[..next.range.start]
                .trim_end_matches([' ', '\t'])
                .ends_with(['\n', '\r']);
            if matches!(next.kind, Kind::Comment) && on_same_line {
                parts.push(text(" "));
            } else {
                parts.push(hardline());
                if is_next_line_empty(self.source, node.range.end) {
                    parts.push(hardline());
                }
            }
        }
        concat(parts)
    }

    fn print_block(&self, nodes: &[Node], at_rule: Option<&str>, soft: bool) -> Doc {
        let break_line = || if soft { softline() } else { hardline() };
        concat(vec![
            text("{"),
            if nodes.is_empty() {
                concat(Vec::new())
            } else {
                indent(vec![break_line(), self.print_sequence(nodes, at_rule)])
            },
            break_line(),
            text("}"),
        ])
    }

    fn print_node(&self, node: &Node, at_rule: Option<&str>) -> Doc {
        match &node.kind {
            Kind::Comment => text(self.slice(node.range)),
            Kind::Rule { selector, nodes } => {
                let context = selector::Context {
                    single_quote: self.single_quote,
                    in_keyframes: at_rule.is_some_and(|name| name.ends_with("keyframes")),
                };
                concat(vec![
                    selector::print(self.slice(*selector), &context),
                    text(" "),
                    self.print_block(nodes, at_rule, false),
                ])
            }
            Kind::Declaration {
                hack,
                prop,
                between,
                value,
                important,
                block,
            } => self.print_declaration(
                hack,
                prop,
                between,
                *value,
                *important,
                block.as_deref(),
                at_rule,
            ),
            Kind::AtRule {
                name,
                params,
                nodes,
            } => self.print_at_rule(name, *params, nodes.as_deref()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn print_declaration(
        &self,
        hack: &str,
        prop: &str,
        between: &str,
        value: Range,
        important: bool,
        block: Option<&[Node]>,
        at_rule: Option<&str>,
    ) -> Doc {
        let raw_value = self.slice(value);
        let lowercase_prop = prop.to_lowercase();
        let trimmed_between = between.trim();
        let printed_value = if let Some(nodes) = block {
            // `--a: { b: c }`
            self.print_block(nodes, at_rule, false)
        } else if raw_value.trim().is_empty() || raw_value.starts_with("progid:") {
            text(raw_value)
        } else {
            let context = value::Context {
                source: self.source,
                offset: value.start,
                prop: Some(&lowercase_prop),
                at_rule,
                single_quote: self.single_quote,
            };
            value::print(raw_value, &context).unwrap_or_else(|| text(raw_value))
        };
        let is_value_all_space = block.is_none() && raw_value.chars().all(|c| c == ' ');
        concat(vec![
            text(hack),
            text(maybe_to_lower_case(prop)),
            text(trimmed_between),
            text(if is_value_all_space { "" } else { " " }),
            printed_value,
            text(if important { " !important" } else { "" }),
            text(";"),
        ])
    }

    fn print_at_rule(&self, name: &str, params: Range, nodes: Option<&[Node]>) -> Doc {
        let lowercase_name = name.to_lowercase();
        let raw_params = self.slice(params);
        let value_context = value::Context {
            source: self.source,
            offset: params.start,
            prop: None,
            at_rule: Some(&lowercase_name),
            single_quote: self.single_quote,
        };
        let printed_params = if raw_params.is_empty() {
            concat(Vec::new())
        } else if VALUE_AT_RULES.contains(&name) {
            // `not (a)` is read as the function `not`, which is then printed with its space.
            let params = glue_first_function(raw_params);
            let context = value::Context {
                source: &params,
                offset: 0,
                ..value_context
            };
            match value::print(&params, &context) {
                Some(value) => group(vec![text(" "), value]),
                None => concat(vec![text(" "), text(raw_params)]),
            }
        } else if matches!(lowercase_name.as_str(), "import" | "use" | "forward") {
            concat(vec![
                text(" "),
                value::print(raw_params, &value_context).unwrap_or_else(|| text(raw_params)),
            ])
        } else if matches!(lowercase_name.as_str(), "media" | "custom-media") {
            concat(vec![text(" "), media::print(raw_params, self.single_quote)])
        } else {
            concat(vec![text(" "), text(raw_params)])
        };
        concat(vec![
            text("@"),
            text(if name.ends_with(':') {
                name.to_string()
            } else {
                maybe_to_lower_case(name)
            }),
            printed_params,
            match nodes {
                Some(nodes) => concat(vec![
                    text(" "),
                    self.print_block(nodes, Some(&lowercase_name), true),
                ]),
                None => text(";"),
            },
        ])
    }
}

/// What a comment says, without its delimiters.
fn comment_text(comment: &str) -> &str {
    comment
        .strip_prefix("/*")
        .and_then(|comment| comment.strip_suffix("*/"))
        .unwrap_or(comment)
        .trim()
}

/// `name (` → `name( `, for the first word of the parameters of an at-rule, unless it is
/// `if`: `/^(?!if)([^"'\s(]+)(\s+)\(/`.
fn glue_first_function(params: &str) -> String {
    let word_end = params
        .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '('))
        .unwrap_or(params.len());
    let after_word = &params[word_end..];
    let spaces = after_word.len() - after_word.trim_start().len();
    if word_end == 0
        || spaces == 0
        || params.starts_with("if")
        || !after_word[spaces..].starts_with('(')
    {
        return params.to_string();
    }
    format!(
        "{}({}{}",
        &params[..word_end],
        &after_word[..spaces],
        &after_word[spaces + 1..]
    )
}

/// The document of a style sheet, or `None` when it is one PostCSS refuses: it is then
/// printed as it was written.
pub(crate) fn format(source: &str, options: &Options) -> Option<Doc> {
    let nodes = parser::parse(source)?;
    let printer = Printer {
        source,
        single_quote: options.embedded_single_quote,
    };
    Some(printer.print_sequence(&nodes, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{PrintOptions, print};

    fn fmt(source: &str) -> String {
        print(
            format(source, &Options::default()).unwrap(),
            &PrintOptions {
                print_width: 80,
                tab_width: 2,
                use_tabs: false,
            },
        )
    }

    #[test]
    fn formats_rules_and_declarations() {
        assert_eq!(
            fmt("a,b>c{COLOR:red;margin:0 AUTO!important}"),
            "a,\nb > c {\n  color: red;\n  margin: 0 AUTO !important;\n}"
        );
        assert_eq!(fmt(".a{}"), ".a {\n}");
    }

    #[test]
    fn keeps_one_blank_line_and_trailing_comments() {
        assert_eq!(
            fmt(".a{color:red; /* why */\n\n\n  margin:0}\n/* b */\n.c{}"),
            ".a {\n  color: red; /* why */\n\n  margin: 0;\n}\n/* b */\n.c {\n}"
        );
    }

    #[test]
    fn formats_at_rules() {
        assert_eq!(
            fmt("@media (min-width:100px) and (max-width:200px){.a{color:red}}"),
            "@media (min-width: 100px) and (max-width: 200px) {\n  .a {\n    color: red;\n  }\n}"
        );
        assert_eq!(
            fmt("@import url(\"a.css\") screen;"),
            "@import url('a.css') screen;"
        );
        assert_eq!(
            fmt("@supports not (display:grid){a{b:c}}"),
            "@supports not (display: grid) {\n  a {\n    b: c;\n  }\n}"
        );
        assert_eq!(
            fmt("@keyframes Foo{FROM{opacity:0}50.0%{opacity:.5}}"),
            "@keyframes Foo {\n  from {\n    opacity: 0;\n  }\n  50.0% {\n    opacity: 0.5;\n  }\n}"
        );
        assert_eq!(
            fmt("@container a (min-width:1px){b{c:d}}"),
            "@container a (min-width:1px) {\n  b {\n    c: d;\n  }\n}"
        );
    }

    #[test]
    fn formats_nested_rules_and_custom_properties() {
        assert_eq!(
            fmt(".i{&:hover{color:red}--B:  Red;--c:{a:b};--empty:;--blank: ;color: ;}"),
            ".i {\n  &:hover {\n    color: red;\n  }\n  --B: Red;\n  --c: {\n    a: b;\n  };\n  --empty:;\n  --blank: ;\n  color:;\n}"
        );
    }

    #[test]
    fn keeps_the_comments_of_a_declaration_where_they_are() {
        assert_eq!(
            fmt("a{b/* 1 */ : /* 2 */ c /* 3 */;d : e}"),
            "a {\n  b/* 1 */ : /* 2 */ c /* 3 */;\n  d: e;\n}"
        );
    }

    #[test]
    fn gives_up_on_what_postcss_refuses() {
        assert!(format("a{", &Options::default()).is_none());
        assert!(format("a{b}", &Options::default()).is_none());
    }
}
