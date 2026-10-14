//! The markup of Liquid tags and outputs: expressions, filters, arguments.

use crate::ast::{Argument, Expr, Filter, Markup, Variable};
use crate::doc::{Doc, concat, group, if_break, indent, join, line, nothing, text};

use super::Printer;

fn comma_line() -> Doc {
    concat(vec![text(","), line()])
}

impl Printer<'_> {
    /// `truncate` is for the bounds of a range, which are whole numbers.
    pub(crate) fn print_expr(&self, expr: &Expr, truncate: bool) -> Doc {
        match expr {
            Expr::String { value, .. } => {
                let preferred = if self.options.liquid_single_quote {
                    '\''
                } else {
                    '"'
                };
                let quote = match (value.contains(preferred), preferred) {
                    (false, '\'') | (true, '"') => "'",
                    _ => "\"",
                };
                concat(vec![text(quote), text(value.as_str()), text(quote)])
            }
            Expr::Number { value, .. } => {
                let fraction = value
                    .rfind('.')
                    .filter(|dot| {
                        truncate
                            && value.len() > dot + 1
                            && value[dot + 1..].bytes().all(|byte| byte.is_ascii_digit())
                    })
                    .unwrap_or(value.len());
                text(&value[..fraction])
            }
            Expr::Literal { keyword, .. } => text(if keyword == "nil" { "null" } else { keyword }),
            Expr::Range { start, end, .. } => concat(vec![
                text("("),
                self.print_expr(start, true),
                text(".."),
                self.print_expr(end, true),
                text(")"),
            ]),
            Expr::Lookup { name, lookups, .. } => {
                let mut parts = Vec::new();
                if let Some(name) = name
                    && !name.is_empty()
                {
                    parts.push(text(name.as_str()));
                }
                for (index, lookup) in lookups.iter().enumerate() {
                    // `a.b` rather than `a['b']`, where the name allows it.
                    if let Expr::String { value, .. } = lookup {
                        let is_global_string_lookup = index == 0 && name.is_none();
                        let bytes = value.strip_suffix('?').unwrap_or(value).as_bytes();
                        let is_bareword = !bytes.is_empty()
                            && !bytes[0].is_ascii_digit()
                            && bytes
                                .iter()
                                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
                        if !is_global_string_lookup && is_bareword {
                            parts.push(concat(vec![text("."), text(value.as_str())]));
                            continue;
                        }
                    }
                    parts.push(concat(vec![
                        text("["),
                        self.print_expr(lookup, false),
                        text("]"),
                    ]));
                }
                concat(parts)
            }
            Expr::Logical {
                relation,
                left,
                right,
                ..
            } => concat(vec![
                self.print_expr(left, false),
                line(),
                text(relation.as_str()),
                text(" "),
                self.print_expr(right, false),
            ]),
            Expr::Comparison {
                comparator,
                left,
                right,
                ..
            } => group(vec![
                self.print_expr(left, false),
                indent(vec![
                    line(),
                    text(comparator.as_str()),
                    text(" "),
                    self.print_expr(right, false),
                ]),
            ]),
            Expr::Boolean { condition, .. } => self.print_expr(condition, false),
            Expr::Array { elements, .. } => concat(vec![
                text("["),
                join(
                    &comma_line(),
                    elements
                        .iter()
                        .map(|element| self.print_expr(element, false))
                        .collect(),
                ),
                text("]"),
            ]),
        }
    }

    fn print_argument(&self, argument: &Argument) -> Doc {
        match argument {
            Argument::Positional(expr) => self.print_expr(expr, false),
            Argument::Named { name, value, .. } => concat(vec![
                text(name.as_str()),
                text(": "),
                self.print_expr(value, false),
            ]),
        }
    }

    fn print_arguments(&self, args: &[Argument]) -> Vec<Doc> {
        args.iter()
            .map(|argument| self.print_argument(argument))
            .collect()
    }

    fn print_filter(&self, filter: &Filter) -> Doc {
        let mut parts = vec![text("| "), text(filter.name.as_str())];
        if !filter.args.is_empty() {
            let mut printed = self.print_arguments(&filter.args);
            if filter.args[0].is_named() {
                parts.push(text(":"));
                parts.push(indent(vec![line(), join(&comma_line(), printed)]));
            } else {
                // The first argument stays on the line of the filter.
                let rest = printed.split_off(1);
                parts.push(text(": "));
                parts.push(printed.pop().unwrap());
                parts.push(if rest.is_empty() {
                    nothing()
                } else {
                    indent(vec![text(","), line(), join(&comma_line(), rest)])
                });
            }
        }
        group(parts)
    }

    pub(crate) fn print_variable(&self, variable: &Variable) -> Doc {
        let filters = if variable.filters.is_empty() {
            nothing()
        } else {
            concat(vec![
                line(),
                join(
                    &line(),
                    variable
                        .filters
                        .iter()
                        .map(|filter| self.print_filter(filter))
                        .collect(),
                ),
            ])
        };
        concat(vec![self.print_expr(&variable.expression, false), filters])
    }

    /// `, a: 1, b: 2`: the arguments that follow the first thing a tag takes.
    fn print_trailing_arguments(&self, args: &[Argument]) -> Vec<Doc> {
        if args.is_empty() {
            return Vec::new();
        }
        vec![
            text(","),
            line(),
            join(&comma_line(), self.print_arguments(args)),
        ]
    }

    /// The markup of a tag whose markup was parsed.
    pub(crate) fn print_markup(&self, markup: &Markup) -> Doc {
        match markup {
            Markup::Raw(raw) => text(raw.as_str()),
            Markup::Nothing | Markup::Statements(_) => nothing(),
            Markup::Variable(variable) => self.print_variable(variable),
            Markup::Assign { name, value } => concat(vec![
                text(name.as_str()),
                text(" = "),
                self.print_variable(value),
            ]),
            Markup::Cycle { group_name, args } => {
                let mut parts = Vec::new();
                if let Some(group_name) = group_name {
                    parts.push(self.print_expr(group_name, false));
                    parts.push(text(":"));
                }
                let whitespace = if args.len() > 1 { line() } else { text(" ") };
                parts.push(whitespace.clone());
                parts.push(join(
                    &concat(vec![text(","), whitespace]),
                    args.iter().map(|arg| self.print_expr(arg, false)).collect(),
                ));
                concat(parts)
            }
            Markup::For {
                variable_name,
                collection,
                reversed,
                args,
            } => {
                let mut parts = vec![
                    text(variable_name.as_str()),
                    text(" in "),
                    self.print_expr(collection, false),
                ];
                if *reversed {
                    parts.push(line());
                    parts.push(text("reversed"));
                }
                if !args.is_empty() {
                    parts.push(concat(vec![
                        line(),
                        join(&line(), self.print_arguments(args)),
                    ]));
                }
                concat(parts)
            }
            Markup::Paginate {
                collection,
                page_size,
                args,
            } => {
                let mut parts = vec![
                    self.print_expr(collection, false),
                    line(),
                    text("by "),
                    self.print_expr(page_size, false),
                ];
                if !args.is_empty() {
                    parts.push(concat(self.print_trailing_arguments(args)));
                }
                concat(parts)
            }
            Markup::ContentFor {
                content_for_type: name,
                args,
            }
            | Markup::Section { name, args } => {
                let mut parts = vec![self.print_expr(name, false)];
                parts.extend(self.print_trailing_arguments(args));
                concat(parts)
            }
            Markup::Block { name, args } => {
                let name = self.print_expr(name, false);
                if args.is_empty() {
                    return name;
                }
                // The arguments wrap with the tag, and then end with a comma.
                let mut parts = vec![name];
                parts.extend(self.print_trailing_arguments(args));
                parts.push(if_break(text(","), nothing(), None));
                concat(parts)
            }
            Markup::Render {
                snippet,
                variable,
                alias,
                args,
            } => {
                let alias = alias.as_deref().filter(|alias| !alias.is_empty());
                let mut parts = vec![self.print_expr(snippet, false)];
                if let Some(variable) = variable {
                    parts.push(if alias.is_some() { line() } else { text(" ") });
                    parts.push(concat(vec![
                        text(variable.kind.as_str()),
                        text(" "),
                        self.print_expr(&variable.name, false),
                    ]));
                }
                if let Some(alias) = alias {
                    parts.push(text(" as "));
                    parts.push(text(alias));
                }
                parts.extend(self.print_trailing_arguments(args));
                concat(parts)
            }
            Markup::Expression(expr) | Markup::Condition(expr) => self.print_expr(expr, false),
            Markup::Arguments(args) => join(&comma_line(), self.print_arguments(args)),
            Markup::Expressions(values) => join(
                &comma_line(),
                values
                    .iter()
                    .map(|value| self.print_expr(value, false))
                    .collect(),
            ),
        }
    }
}
