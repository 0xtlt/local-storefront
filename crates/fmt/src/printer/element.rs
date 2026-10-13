//! HTML elements: an opening tag, children, a closing tag.

use crate::ast::{HtmlKind, Id, Kind, is_blank};
use crate::doc::{
    Doc, concat, dedent_to_root, group, group_with, indent, line, nothing, replace_end_of_line,
    softline, text,
};

use super::{Args, Gid, Printer};

impl Printer<'_> {
    /// `<script>`, `<style>`, `<svg>`: what they hold is not HTML.
    fn print_raw_element(&self, id: Id) -> Doc {
        let node = self.ast.get(id);
        let attr_group = self.new_group();
        let mut parts = vec![
            self.print_opening_tag_prefix(id),
            group_with(
                self.print_opening_tag(id, attr_group),
                Some(attr_group),
                false,
            ),
        ];
        if let Kind::Html {
            body: Some(body), ..
        } = &node.kind
            && let Kind::RawMarkup { value, .. } = &self.ast.get(*body).kind
            && !is_blank(value)
        {
            parts.push(self.print(*body, &Args::default()));
        }
        parts.push(self.print_closing_tag(id));
        parts.push(self.print_closing_tag_suffix(id));
        group(parts)
    }

    pub(crate) fn print_element(&self, id: Id, _args: &Args) -> Doc {
        let node = self.ast.get(id);
        if node.is_html_kind(HtmlKind::Raw) {
            return self.print_raw_element(id);
        }
        let attr_group = self.new_group();
        let element_group = self.new_group();
        let opening_tag = || {
            group_with(
                self.print_opening_tag(id, attr_group),
                Some(attr_group),
                false,
            )
        };

        if self.has_no_children(id) {
            return concat(vec![
                opening_tag(),
                self.print_closing_tag(id),
                self.print_closing_tag_suffix(id),
            ]);
        }

        if self.should_preserve_content(id) {
            return concat(vec![
                self.print_opening_tag_prefix(id),
                opening_tag(),
                replace_end_of_line(self.node_content(id)),
                self.print_closing_tag(id),
                self.print_closing_tag_suffix(id),
            ]);
        }

        let print_tag = |contents: Doc| {
            group_with(
                vec![opening_tag(), contents, self.print_closing_tag(id)],
                Some(element_group),
                false,
            )
        };
        let has_close_tag = !self.is_html_dangling_marker_open(id);
        let (Some(first), Some(last)) = (node.first_child, node.last_child) else {
            let whitespace = &node.whitespace;
            return print_tag(
                if whitespace.has_dangling && whitespace.is_dangling_sensitive && has_close_tag {
                    line()
                } else {
                    nothing()
                },
            );
        };
        let first_whitespace = &self.ast.get(first).whitespace;
        let last_whitespace = &self.ast.get(last).whitespace;

        let line_before_children =
            if first_whitespace.has_leading && first_whitespace.is_leading_sensitive {
                line()
            } else if self.ast.get(first).is_text()
                && node.whitespace.is_whitespace_sensitive
                && node.whitespace.is_indentation_sensitive
            {
                dedent_to_root(softline())
            } else {
                softline()
            };

        let last_has_sensitive_whitespace =
            last_whitespace.has_trailing && last_whitespace.is_trailing_sensitive;
        let line_after_children = if !has_close_tag {
            nothing()
        } else {
            let needs_to_borrow = match node.next {
                Some(next) => self.needs_to_borrow_prev_closing_tag_end_marker(next),
                None => node.parent.is_some_and(|parent| {
                    self.needs_to_borrow_last_child_closing_tag_end_marker(parent)
                }),
            };
            match (needs_to_borrow, last_has_sensitive_whitespace) {
                (true, true) => text(" "),
                (true, false) => nothing(),
                (false, true) => line(),
                (false, false) => softline(),
            }
        };

        let children_args = Args {
            leading: vec![Gid::Id(element_group)],
            trailing: vec![Gid::Id(element_group)],
            is_liquid_statement: false,
        };
        print_tag(concat(vec![
            if self.force_break_content(id) {
                Doc::BreakParent
            } else {
                nothing()
            },
            indent(vec![
                line_before_children,
                self.print_children(id, &children_args),
            ]),
            line_after_children,
        ]))
    }
}
