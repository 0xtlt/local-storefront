//! Liquid: outputs, tags, their branches, raw tags, and what a `{% doc %}` holds.

use crate::ast::{Expr, Id, Kind, Markup, is_blank, trim, trim_end};
use crate::doc::{
    Doc, concat, group, group_with, hardline, if_break, indent, join, line, literalline, nothing,
    remove_lines, replace_end_of_line, softline, text,
};

use super::utils::{has_line_break_in_range, markup_lines, reindent};
use super::{Args, Gid, Printer, whitespace_trim};

/// The tags that take no markup: what is written after their name is dropped.
const TAGS_WITHOUT_MARKUP: [&str; 9] = [
    "style",
    "schema",
    "javascript",
    "else",
    "break",
    "continue",
    "comment",
    "raw",
    "doc",
];

/// The words of a description, one space apart.
fn normalize_spaces(text: &str) -> String {
    super::utils::words(trim(text)).join(" ")
}

impl Printer<'_> {
    pub(crate) fn print_liquid_variable_output(&self, id: Id, args: &Args) -> Doc {
        let node = self.ast.get(id);
        let whitespace_start = whitespace_trim(
            node.whitespace_start(),
            self.has_meaningful_lack_of_leading_whitespace(id),
            &args.leading,
        );
        let whitespace_end = whitespace_trim(
            node.whitespace_end(),
            self.has_meaningful_lack_of_trailing_whitespace(id),
            &args.trailing,
        );
        match node.markup() {
            Some(Markup::Variable(variable)) => {
                let whitespace = || {
                    if variable.filters.is_empty() {
                        text(" ")
                    } else {
                        line()
                    }
                };
                group(vec![
                    text("{{"),
                    whitespace_start,
                    indent(vec![whitespace(), self.print_variable(variable)]),
                    whitespace(),
                    whitespace_end,
                    text("}}"),
                ])
            }
            markup => {
                // The markup was not understood: it is printed as written.
                let markup = markup.and_then(Markup::raw).unwrap_or_default();
                let lines = markup_lines(markup);
                if lines.len() > 1 {
                    return group(vec![
                        text("{{"),
                        whitespace_start,
                        indent(vec![
                            hardline(),
                            join(
                                &hardline(),
                                lines.into_iter().map(|line| text(trim(line))).collect(),
                            ),
                        ]),
                        hardline(),
                        whitespace_end,
                        text("}}"),
                    ]);
                }
                group(vec![
                    text("{{"),
                    whitespace_start,
                    text(" "),
                    text(markup),
                    text(" "),
                    whitespace_end,
                    text("}}"),
                ])
            }
        }
    }

    /// The opening of a tag or of a branch whose markup was parsed.
    fn print_named_liquid_block_start(
        &self,
        id: Id,
        args: &Args,
        whitespace_start: Doc,
        whitespace_end: Doc,
    ) -> Doc {
        let node = self.ast.get(id);
        let name = node.name().unwrap_or_default();
        let markup = node.markup().unwrap();
        let statement = args.is_liquid_statement;
        // Inside `{% liquid %}` a tag has no delimiters and stays on its line.
        let wrap = |mut parts: Vec<Doc>, trailing_whitespace: Doc| {
            if statement {
                return remove_lines(concat(parts));
            }
            let mut tag = vec![text("{%"), whitespace_start.clone(), text(" ")];
            tag.append(&mut parts);
            tag.extend([trailing_whitespace, whitespace_end.clone(), text("%}")]);
            group(tag)
        };
        let tag = |trailing_whitespace: Doc| {
            wrap(
                vec![text(name), text(" "), indent(self.print_markup(markup))],
                trailing_whitespace,
            )
        };
        let line_if = |breaks: bool| if breaks { line() } else { text(" ") };

        match markup {
            Markup::Variable(variable) => tag(line_if(!variable.filters.is_empty())),
            Markup::Assign { value, .. } => tag(line_if(!value.filters.is_empty())),
            Markup::Cycle { group_name, args } => wrap(
                vec![
                    text(name),
                    // The line breaks after the name of the group.
                    text(if group_name.is_some() { " " } else { "" }),
                    indent(self.print_markup(markup)),
                ],
                line_if(args.len() > 1),
            ),
            Markup::ContentFor { args, .. } | Markup::Block { args, .. } => {
                tag(line_if(!args.is_empty()))
            }
            Markup::Render {
                variable,
                alias,
                args,
                ..
            } => tag(line_if(
                !args.is_empty() || (variable.is_some() && alias.is_some()),
            )),
            Markup::Expression(_) | Markup::Section { .. } => tag(text(" ")),
            Markup::Nothing => wrap(vec![text(name)], text(" ")),
            Markup::Arguments(values) => tag(line_if(values.len() > 1)),
            Markup::Expressions(values) => tag(line_if(values.len() > 1)),
            Markup::For { reversed, args, .. } => tag(line_if(*reversed || !args.is_empty())),
            Markup::Paginate { .. } => tag(line()),
            Markup::Condition(condition) => tag(line_if(matches!(
                condition,
                Expr::Comparison { .. } | Expr::Logical { .. }
            ))),
            Markup::Statements(statements) => {
                let statement_args = Args {
                    is_liquid_statement: true,
                    ..args.clone()
                };
                let printed = statements
                    .iter()
                    .map(|statement| {
                        concat(vec![
                            self.space_between_lines(*statement),
                            self.print(*statement, &statement_args),
                        ])
                    })
                    .collect();
                let mut parts = Vec::new();
                if !statement {
                    parts.extend([text("{%"), whitespace_start, text(" ")]);
                }
                parts.push(text(name));
                parts.push(indent(vec![hardline(), join(&hardline(), printed)]));
                if !statement {
                    parts.extend([hardline(), whitespace_end, text("%}")]);
                }
                group(parts)
            }
            Markup::Raw(_) => nothing(),
        }
    }

    /// A blank line between two tags of a `{% liquid %}` tag is kept.
    fn space_between_lines(&self, statement: Id) -> Doc {
        match self.ast.get(statement).prev {
            Some(prev) if self.has_more_than_one_new_line_between(prev, statement) => hardline(),
            _ => nothing(),
        }
    }

    pub(crate) fn print_liquid_block_start(&self, id: Id, args: &Args) -> Doc {
        let node = self.ast.get(id);
        let Some(name) = node.name() else {
            return nothing();
        };
        let whitespace_start = whitespace_trim(
            node.whitespace_start(),
            self.needs_block_start_leading_whitespace_stripping_on_break(id),
            &args.leading,
        );
        let whitespace_end = whitespace_trim(
            node.whitespace_end(),
            self.needs_block_start_trailing_whitespace_stripping_on_break(id),
            &args.trailing,
        );
        let Some(Markup::Raw(markup)) = node.markup() else {
            return self.print_named_liquid_block_start(id, args, whitespace_start, whitespace_end);
        };

        if args.is_liquid_statement {
            let skip_leading_space = is_blank(markup) || (name == "#" && markup.starts_with('#'));
            return concat(vec![
                text(name),
                text(if skip_leading_space { "" } else { " " }),
                text(markup.as_str()),
            ]);
        }

        let lines = markup_lines(markup);
        if name == "liquid" {
            return group(vec![
                text("{%"),
                whitespace_start,
                text(" "),
                text(name),
                indent(vec![
                    hardline(),
                    join(
                        &hardline(),
                        reindent(lines, true).into_iter().map(text).collect(),
                    ),
                ]),
                hardline(),
                whitespace_end,
                text("%}"),
            ]);
        }
        if lines.len() > 1 {
            return group(vec![
                text("{%"),
                whitespace_start,
                indent(vec![
                    hardline(),
                    text(name),
                    text(" "),
                    join(
                        &hardline(),
                        lines.into_iter().map(|line| text(trim(line))).collect(),
                    ),
                ]),
                hardline(),
                whitespace_end,
                text("%}"),
            ]);
        }
        let printed_markup = if !markup.is_empty() && !TAGS_WITHOUT_MARKUP.contains(&name) {
            format!(" {markup}")
        } else {
            String::new()
        };
        group(vec![
            text("{%"),
            whitespace_start,
            text(" "),
            text(name),
            text(printed_markup),
            text(" "),
            whitespace_end,
            text("%}"),
        ])
    }

    fn print_liquid_block_end(&self, id: Id, args: &Args) -> Doc {
        let node = self.ast.get(id);
        if node.children().is_none() || node.block_end().is_none() {
            return nothing();
        }
        let name = node.name().unwrap_or_default();
        if args.is_liquid_statement {
            return concat(vec![text("end"), text(name)]);
        }
        let (delimiter_start, delimiter_end) = node.delimiter_whitespace().unwrap_or_default();
        let whitespace_start = whitespace_trim(
            delimiter_start,
            self.needs_block_end_leading_whitespace_stripping_on_break(id),
            &args.leading,
        );
        let whitespace_end = whitespace_trim(
            delimiter_end,
            self.has_meaningful_lack_of_trailing_whitespace(id),
            &args.trailing,
        );
        group(vec![
            text("{%"),
            whitespace_start,
            text(format!(" end{name} ")),
            whitespace_end,
            text("%}"),
        ])
    }

    pub(crate) fn print_liquid_tag(&self, id: Id, args: &Args) -> Doc {
        let node = self.ast.get(id);
        let (Some(children), Some(block_end)) = (node.children(), node.block_end()) else {
            return self.print_liquid_block_start(id, args);
        };
        let block_start = node.block_start().unwrap();
        let name = node.name().unwrap_or_default();

        if !args.is_liquid_statement && self.should_preserve_content(id) {
            // `capture`: what it holds is kept as written.
            return concat(vec![
                self.print_liquid_block_start(
                    id,
                    &args.with_groups(args.leading.clone(), vec![Gid::ForceFlat]),
                ),
                replace_end_of_line(self.src(block_start.end, block_end.start)),
                self.print_liquid_block_end(
                    id,
                    &args.with_groups(vec![Gid::ForceFlat], args.trailing.clone()),
                ),
            ]);
        }

        let tag_group = self.new_group();
        // An empty tag has no whitespace inside it to strip.
        let inner_group = if children.is_empty() {
            Gid::ForceFlat
        } else {
            Gid::Id(tag_group)
        };
        let block_start_doc = self.print_liquid_block_start(
            id,
            &args.with_groups(args.leading.clone(), vec![inner_group]),
        );
        let block_end_doc = self.print_liquid_block_end(
            id,
            &args.with_groups(vec![inner_group], args.trailing.clone()),
        );

        let children_args = args.with_groups(vec![Gid::Id(tag_group)], vec![Gid::Id(tag_group)]);
        let body = if self.ast.is_branched_tag(id) || name == "tablerow" {
            let branches: Vec<Doc> = children
                .iter()
                .map(|child| self.print(*child, &children_args))
                .filter(|doc| !doc.is_empty_text())
                .collect();
            if name == "case" {
                indent(branches)
            } else {
                concat(branches)
            }
        } else if !children.is_empty() {
            indent(vec![
                self.inner_leading_whitespace(id),
                self.print_children(id, &children_args),
            ])
        } else {
            concat(Vec::new())
        };

        let should_break = matches!(name, "for" | "case")
            || has_line_break_in_range(self.source, node.position.start, node.position.end)
            || self.ast.is_attribute_node(id)
            || self.is_deeply_nested(id);
        group_with(
            vec![
                block_start_doc,
                body,
                self.inner_trailing_whitespace(id, args),
                block_end_doc,
            ],
            Some(tag_group),
            should_break,
        )
    }

    pub(crate) fn print_liquid_raw_tag(&self, id: Id, args: &Args) -> Doc {
        let node = self.ast.get(id);
        let Kind::LiquidRawTag {
            name,
            markup,
            body,
            block_start,
            block_end,
            ..
        } = &node.kind
        else {
            return nothing();
        };
        let dash = |trims: bool| text(if trims { "-" } else { "" });
        let body_node = self.ast.get(*body);
        let Kind::RawMarkup { value, .. } = &body_node.kind else {
            return nothing();
        };
        let should_print_as_is = node.whitespace.is_indentation_sensitive
            || !has_line_break_in_range(
                self.source,
                body_node.position.start,
                body_node.position.end,
            );

        let (start, end) = if args.is_liquid_statement {
            (
                text(name.as_str()),
                concat(vec![text("end"), text(name.as_str())]),
            )
        } else {
            let printed_markup =
                if !markup.is_empty() && !TAGS_WITHOUT_MARKUP.contains(&name.as_str()) {
                    format!("{markup} ")
                } else {
                    String::new()
                };
            (
                group(vec![
                    text("{%"),
                    dash(node.whitespace_start()),
                    text(" "),
                    text(name.as_str()),
                    text(" "),
                    text(printed_markup),
                    dash(node.whitespace_end()),
                    text("%}"),
                ]),
                // The end tag takes the whitespace control of the opening one.
                concat(vec![
                    text("{%"),
                    dash(node.whitespace_start()),
                    text(" end"),
                    text(name.as_str()),
                    text(" "),
                    dash(node.whitespace_end()),
                    text("%}"),
                ]),
            )
        };

        let body_doc = if should_print_as_is {
            text(self.src(block_start.end, block_end.start))
        } else if is_blank(value) {
            hardline()
        } else {
            self.print(*body, &Args::default())
        };
        concat(vec![start, body_doc, end])
    }

    /// What a `{% doc %}` holds: one annotation per line, a blank line between two kinds.
    pub(crate) fn print_liquid_doc(&self, id: Id) -> Doc {
        let Kind::RawMarkup { nodes, .. } = &self.ast.get(id).kind else {
            return nothing();
        };
        if nodes.is_empty() {
            return concat(Vec::new());
        }
        // What tells two kinds apart is the first thing each prints.
        let key = |node: Id| match &self.ast.get(node).kind {
            Kind::DocParam { .. } => "@param".to_string(),
            Kind::DocExample { .. } => "@example".to_string(),
            Kind::DocPrompt { .. } => "@prompt".to_string(),
            Kind::DocDescription {
                implicit: false, ..
            } => "@description".to_string(),
            Kind::DocDescription { content, .. } => trim(content).to_string(),
            _ => String::new(),
        };
        let mut lines = Vec::new();
        for (index, node) in nodes.iter().enumerate() {
            if index > 0 {
                lines.push(hardline());
                if key(nodes[index - 1]) != key(*node) {
                    lines.push(hardline());
                }
            }
            lines.push(self.print(*node, &Args::default()));
        }
        concat(vec![indent(vec![hardline(), concat(lines)]), hardline()])
    }

    pub(crate) fn print_liquid_doc_param(&self, id: Id) -> Doc {
        let Kind::DocParam {
            name,
            description,
            param_type,
            required,
        } = &self.ast.get(id).kind
        else {
            return nothing();
        };
        if name.is_empty() {
            // A `@param` that could not be read is kept as written.
            let raw = self.node_source(id);
            return concat(vec![
                text("@param"),
                text(raw.get("@param".len()..).unwrap_or_default()),
            ]);
        }
        let mut parts = vec![text("@param")];
        if let Some(param_type) = param_type.as_deref().filter(|kind| !kind.is_empty()) {
            parts.push(text(format!(" {{{}}}", trim(param_type))));
        }
        parts.push(text(if *required {
            format!(" {}", trim(name))
        } else {
            format!(" [{}]", trim(name))
        }));
        if let Some(description) = description
            .as_deref()
            .filter(|description| !description.is_empty())
        {
            let separator = if self.options.liquid_doc_param_dash {
                " - "
            } else {
                " "
            };
            parts.push(text(format!(
                "{separator}{}",
                normalize_spaces(description)
            )));
        }
        concat(parts)
    }

    pub(crate) fn print_liquid_doc_content(annotation: &str, content: &str, inline: bool) -> Doc {
        concat(vec![
            text(annotation),
            if trim_end(content).contains('\n') || !inline {
                hardline()
            } else {
                text(" ")
            },
            text(trim(content)),
        ])
    }

    fn inner_leading_whitespace(&self, id: Id) -> Doc {
        let node = self.ast.get(id);
        let Some(first) = node.first_child else {
            return if node.whitespace.is_dangling_sensitive && node.whitespace.has_dangling {
                line()
            } else {
                nothing()
            };
        };
        let whitespace = &self.ast.get(first).whitespace;
        if whitespace.has_leading && whitespace.is_leading_sensitive {
            line()
        } else {
            softline()
        }
    }

    fn inner_trailing_whitespace(&self, id: Id, args: &Args) -> Doc {
        let node = self.ast.get(id);
        let Some(last) = node.last_child else {
            return nothing();
        };
        if (!args.is_liquid_statement && self.should_preserve_content(id))
            || node.is_branch()
            || node.block_end().is_none()
        {
            return nothing();
        }
        let whitespace = &self.ast.get(last).whitespace;
        if whitespace.has_trailing && whitespace.is_trailing_sensitive {
            line()
        } else {
            softline()
        }
    }

    /// Whether what follows the last child of a branch, up to the next branch, is a blank
    /// line, which is then kept.
    fn should_add_trailing_newline(&self, branch: Id) -> bool {
        let node = self.ast.get(branch);
        let (Some(next), Some(last)) = (node.next, node.children().and_then(<[Id]>::last)) else {
            return false;
        };
        let between = self.src(
            self.ast.get(*last).position.end,
            self.ast.get(next).position.start,
        );
        between.chars().filter(|c| *c != ' ' && *c != '\t').count() >= 2
    }

    pub(crate) fn print_liquid_branch(&self, id: Id, args: &Args) -> Doc {
        let node = self.ast.get(id);
        let children = node.children().unwrap_or_default();
        let trailing_newline = || {
            if self.should_add_trailing_newline(id) {
                literalline()
            } else {
                nothing()
            }
        };

        if node.name().is_none() {
            // What comes before the first `else`, `elsif` or `when`.
            let parent = node.parent.unwrap();
            let parent_children = self.ast.get(parent).children().unwrap_or_default();
            // `{% if a %} {% endif %}`: the space is the one before the end tag.
            if children.is_empty() && parent_children.len() == 1 {
                return nothing();
            }
            if children.is_empty() && !node.whitespace.has_dangling {
                return nothing();
            }
            // `{% if a %} {% else %}b{% endif %}`
            if node.whitespace.has_dangling {
                return if_break(nothing(), text(" "), None);
            }
            return indent(vec![
                self.inner_leading_whitespace(parent),
                self.print_children(id, args),
                trailing_newline(),
            ]);
        }

        // The whitespace after an empty branch is the one before this one.
        let prev_is_empty = node
            .prev
            .is_some_and(|prev| self.ast.get(prev).children().is_some_and(<[Id]>::is_empty));
        let outer_leading_whitespace = if node.whitespace.has_leading && !prev_is_empty {
            line()
        } else {
            softline()
        };
        concat(vec![
            outer_leading_whitespace,
            self.print_liquid_block_start(id, args),
            indent(vec![
                self.inner_leading_whitespace(id),
                self.print_children(id, args),
                trailing_newline(),
            ]),
        ])
    }

    fn needs_block_start_leading_whitespace_stripping_on_break(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        let among_attributes = if node.is_branch() {
            node.parent
                .is_some_and(|parent| self.ast.is_attribute_node(parent))
        } else {
            self.ast.is_attribute_node(id)
        };
        !among_attributes && self.has_meaningful_lack_of_leading_whitespace(id)
    }

    fn needs_block_start_trailing_whitespace_stripping_on_break(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        if node.is_branch() {
            if node
                .parent
                .is_some_and(|parent| self.ast.is_attribute_node(parent))
            {
                return false;
            }
            return match node.first_child {
                Some(first) => self.has_meaningful_lack_of_leading_whitespace(first),
                None => self.has_meaningful_lack_of_dangling_whitespace(id),
            };
        }
        if self.ast.is_branched_tag(id)
            && let Some(first) = node.first_child
        {
            return self.needs_block_start_leading_whitespace_stripping_on_break(first);
        }
        match (node.children(), node.first_child) {
            (None, _) => self.has_meaningful_lack_of_trailing_whitespace(id),
            (Some(_), Some(first)) => self.has_meaningful_lack_of_leading_whitespace(first),
            (Some(_), None) => self.has_meaningful_lack_of_dangling_whitespace(id),
        }
    }

    fn needs_block_end_leading_whitespace_stripping_on_break(&self, id: Id) -> bool {
        if self.ast.is_attribute_node(id) {
            return false;
        }
        match self.ast.get(id).last_child {
            Some(last) => self.has_meaningful_lack_of_trailing_whitespace(last),
            None => self.has_meaningful_lack_of_dangling_whitespace(id),
        }
    }
}
