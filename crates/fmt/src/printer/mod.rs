//! Tree → document. A port of the printer of Shopify's Prettier plugin for Liquid
//! (`@shopify/prettier-plugin-liquid`), function for function, so that the same tree gives the
//! same document.

mod children;
mod element;
mod expression;
mod liquid;
mod tag;
mod utils;

use std::cell::Cell;

use crate::ast::{Ast, AttrKind, Id, Kind, RawKind, is_blank, trim, trim_end};
use crate::doc::{
    Doc, GroupId, concat, dedent_to_root, fill, group, group_with, hardline, if_break, indent,
    join, line, nothing, remove_lines, replace_end_of_line, softline, text,
};
use crate::embed;
use crate::options::Options;

use utils::{body_lines, reindent};

/// Which group decides whether the whitespace next to a Liquid delimiter is stripped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Gid {
    /// The group being printed, whichever it is.
    Enclosing,
    Id(GroupId),
    /// The whitespace never becomes a line break.
    ForceFlat,
    /// The whitespace is always a line break.
    ForceBreak,
}

#[derive(Debug, Clone)]
pub(crate) struct Args {
    pub leading: Vec<Gid>,
    pub trailing: Vec<Gid>,
    /// Inside a `{% liquid %}` tag: tags are printed without their delimiters.
    pub is_liquid_statement: bool,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            leading: vec![Gid::Enclosing],
            trailing: vec![Gid::Enclosing],
            is_liquid_statement: false,
        }
    }
}

impl Args {
    fn with_groups(&self, leading: Vec<Gid>, trailing: Vec<Gid>) -> Args {
        Args {
            leading,
            trailing,
            is_liquid_statement: self.is_liquid_statement,
        }
    }
}

pub(crate) struct Printer<'a> {
    pub ast: &'a Ast,
    pub source: &'a str,
    pub options: &'a Options,
    next_group: Cell<GroupId>,
}

/// Threads `ifBreak` through every group that may break next to a delimiter: `-` when one of
/// them does and whitespace must not appear there.
pub(crate) fn if_break_chain(breaks: &str, flat: &str, groups: &[Gid]) -> Doc {
    if groups.contains(&Gid::ForceBreak) {
        return text(breaks);
    }
    if groups.contains(&Gid::ForceFlat) {
        return text(flat);
    }
    groups.iter().fold(text(flat), |flat, group| {
        let id = match group {
            Gid::Id(id) => Some(*id),
            _ => None,
        };
        if_break(text(breaks), flat, id)
    })
}

/// The `-` of a delimiter: the one written, or one added when the line breaks there.
pub(crate) fn whitespace_trim(current: bool, strip_on_break: bool, groups: &[Gid]) -> Doc {
    let current = if current { "-" } else { "" };
    if_break_chain(if strip_on_break { "-" } else { current }, current, groups)
}

impl<'a> Printer<'a> {
    pub fn new(ast: &'a Ast, source: &'a str, options: &'a Options) -> Printer<'a> {
        Printer {
            ast,
            source,
            options,
            next_group: Cell::new(1),
        }
    }

    pub(crate) fn new_group(&self) -> GroupId {
        let id = self.next_group.get();
        self.next_group.set(id + 1);
        id
    }

    pub(crate) fn src(&self, start: usize, end: usize) -> &'a str {
        &self.source[start.min(end)..end]
    }

    pub(crate) fn node_source(&self, id: Id) -> &'a str {
        let position = self.ast.get(id).position;
        self.src(position.start, position.end)
    }

    pub fn print_root(&self) -> Doc {
        self.print(self.ast.root, &Args::default())
    }

    pub(crate) fn print(&self, id: Id, args: &Args) -> Doc {
        let node = self.ast.get(id);
        match &node.kind {
            Kind::Document { .. } => concat(vec![self.print_children(id, args), hardline()]),
            Kind::Html { .. } => self.print_element(id, args),
            Kind::RawMarkup { .. } => self.print_raw_markup(id),
            Kind::LiquidVariableOutput { .. } => self.print_liquid_variable_output(id, args),
            Kind::LiquidRawTag { .. } => self.print_liquid_raw_tag(id, args),
            Kind::LiquidTag { .. } => self.print_liquid_tag(id, args),
            Kind::LiquidBranch { .. } => self.print_liquid_branch(id, args),
            Kind::Attr { kind, .. } => {
                if *kind == AttrKind::Empty {
                    self.print_attribute_name(id)
                } else {
                    self.print_attribute(id)
                }
            }
            Kind::HtmlDoctype { legacy } => match legacy.as_deref() {
                None | Some("") => text("<!doctype html>"),
                Some(_) => text(self.node_source(id)),
            },
            Kind::HtmlComment { body } => self.print_html_comment(id, body),
            Kind::Text { .. } => self.print_text_node(id),
            Kind::YamlFrontmatter { body } => concat(vec![
                text("---"),
                hardline(),
                text(body.as_str()),
                text("---"),
            ]),
            Kind::DocParam { .. } => self.print_liquid_doc_param(id),
            Kind::DocExample { content, inline } => {
                Self::print_liquid_doc_content("@example", content, *inline)
            }
            Kind::DocDescription {
                content,
                implicit,
                inline,
            } => {
                if *implicit {
                    concat(vec![text(trim(content))])
                } else {
                    Self::print_liquid_doc_content("@description", content, *inline)
                }
            }
            Kind::DocPrompt { content } => concat(vec![text("@prompt"), text(trim_end(content))]),
            Kind::Error => text(self.node_source(id)),
        }
    }

    fn print_attribute_name(&self, id: Id) -> Doc {
        let Kind::Attr { name, .. } = &self.ast.get(id).kind else {
            return nothing();
        };
        // What Liquid a name holds stays on one line.
        concat(
            name.iter()
                .map(|part| remove_lines(self.print(*part, &Args::default())))
                .collect(),
        )
    }

    fn print_attribute(&self, id: Id) -> Doc {
        let Kind::Attr {
            value,
            attribute_position,
            ..
        } = &self.ast.get(id).kind
        else {
            return nothing();
        };
        // The value is printed as it was written: there is no telling how an attribute that
        // holds Liquid may be laid out without changing what it renders.
        let source = self.src(attribute_position.start, attribute_position.end);
        let preferred = if self.options.single_quote { '\'' } else { '"' };
        let value_holds_quote = value.iter().any(|part| {
            self.ast
                .get(*part)
                .text()
                .is_some_and(|text| text.contains(preferred))
        });
        let quote = match (value_holds_quote, preferred) {
            (false, '"') | (true, '\'') => "\"",
            _ => "'",
        };
        let printed_value = if source.contains('\n') {
            let lines = reindent(body_lines(source), true);
            group_with(
                vec![
                    indent(vec![
                        softline(),
                        join(&hardline(), lines.into_iter().map(text).collect()),
                    ]),
                    softline(),
                ],
                Some(self.new_group()),
                false,
            )
        } else {
            text(source)
        };
        concat(vec![
            self.print_attribute_name(id),
            text("="),
            text(quote),
            printed_value,
            text(quote),
        ])
    }

    fn print_text_node(&self, id: Id) -> Doc {
        let node = self.ast.get(id);
        let value = node.text().unwrap_or_default();
        let is_front_matter = node.prev.is_none()
            && self
                .ast
                .parent(id)
                .is_some_and(|parent| matches!(parent.kind, Kind::Document { .. }))
            && (value.starts_with("---\n") || value.starts_with("---\r\n"));
        if is_front_matter {
            return text(value);
        }
        if is_blank(value) {
            return nothing();
        }
        if self.is_pre_like(id) {
            return concat(vec![
                self.print_opening_tag_prefix(id),
                replace_end_of_line(value),
                self.print_closing_tag_suffix(id),
            ]);
        }
        let paragraphs = utils::paragraphs(value)
            .into_iter()
            .map(|paragraph| {
                let mut words = Vec::new();
                for (index, word) in utils::words(trim(paragraph)).into_iter().enumerate() {
                    if index > 0 {
                        words.push(line());
                    }
                    words.push(text(word));
                }
                fill(words)
            })
            .collect();
        concat(vec![
            self.print_opening_tag_prefix(id),
            join(&hardline(), paragraphs),
            self.print_closing_tag_suffix(id),
        ])
    }

    fn print_html_comment(&self, id: Id, body: &str) -> Doc {
        let source = self.node_source(id);
        let lines_of = |body: &str| {
            join(
                &hardline(),
                reindent(body_lines(body), true)
                    .into_iter()
                    .map(text)
                    .collect(),
            )
        };
        if let Some((start_tag, inner, end_tag)) = utils::conditional_comment(source) {
            return concat(vec![
                text(start_tag),
                group(vec![indent(vec![line(), lines_of(inner)]), line()]),
                text(end_tag),
            ]);
        }
        if body.contains("prettier-ignore")
            || body.starts_with("display:")
            || body.starts_with("white-space:")
        {
            return text(source);
        }
        concat(vec![
            text("<!--"),
            group(vec![indent(vec![line(), lines_of(body)]), line()]),
            text("-->"),
        ])
    }

    /// What a raw tag or a raw element holds: formatted in its own language when there is a
    /// formatter for it and it parses, otherwise its lines, indented again.
    fn print_raw_markup(&self, id: Id) -> Doc {
        let node = self.ast.get(id);
        let Kind::RawMarkup { kind, value, .. } = &node.kind else {
            return nothing();
        };
        let parent_name = self.ast.parent(id).and_then(|parent| {
            matches!(parent.kind, Kind::LiquidRawTag { .. })
                .then(|| parent.name())
                .flatten()
        });
        let should_indent_body = *kind != RawKind::Markdown
            && !(parent_name == Some("schema") && !self.options.indent_schema);
        let wrap = |body: Doc| {
            if should_indent_body {
                concat(vec![indent(vec![hardline(), body]), hardline()])
            } else {
                concat(vec![dedent_to_root(vec![hardline(), body]), hardline()])
            }
        };

        if self.options.embedded_language_formatting
            && !is_blank(value)
            && let Some(body) = embed::format(*kind, value, self.options)
        {
            return wrap(body);
        }

        if parent_name == Some("doc") {
            return self.print_liquid_doc(id);
        }
        // The indentation of what a template literal holds is part of the string.
        if *kind == RawKind::Javascript && value.contains('`') {
            return concat(vec![text(trim_end(value)), hardline()]);
        }
        let lines = body_lines(value);
        let first_line = value.split('\n').next().unwrap_or_default();
        let skip_first_line = !is_blank(first_line);
        let body = if lines.iter().any(|line| !is_blank(line)) {
            join(
                &hardline(),
                reindent(lines, skip_first_line)
                    .into_iter()
                    .map(text)
                    .collect(),
            )
        } else {
            softline()
        };
        wrap(body)
    }

    pub(crate) fn is_pre_like(&self, id: Id) -> bool {
        self.ast.get(id).css_whitespace.starts_with("pre")
    }
}
