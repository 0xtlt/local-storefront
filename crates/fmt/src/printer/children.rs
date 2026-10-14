//! The children of a node, and the whitespace between them: a line break where the source
//! had one and it changes nothing, a space or nothing where whitespace would show in the page.

use crate::ast::{HtmlKind, Id};
use crate::doc::{
    Doc, concat, group_with, hardline, if_break, line, nothing, replace_end_of_line, softline,
};

use super::{Args, Gid, Printer};

/// What separates two children.
#[derive(Clone, Copy, PartialEq)]
enum Between {
    Nothing,
    Hardline,
    Line,
    Softline,
}

/// The whitespace around one child, sorted by what makes it a line break.
#[derive(Default)]
struct WhitespaceAround {
    /// Line breaks before the child, whatever the child does.
    leading_hardlines: usize,
    /// Breaks first when the child does not fit.
    leading: Vec<Doc>,
    /// Breaks with the trailing whitespace when the child does not fit.
    leading_dependent: Vec<Doc>,
    /// Breaks when the child breaks.
    trailing: Vec<Doc>,
    /// Line breaks after the child, whatever the child does.
    trailing_hardlines: usize,
}

impl Printer<'_> {
    fn print_child(&self, child: Id, args: &Args) -> Doc {
        if !self.has_prettier_ignore(child) {
            return self.print(child, args);
        }
        // Printed as written, without the markers its neighbours print for it.
        let node = self.ast.get(child);
        let mut start = node.position.start;
        if node
            .prev
            .is_some_and(|prev| self.needs_to_borrow_next_opening_tag_start_marker(prev))
        {
            start += self.opening_tag_start_marker(Some(child)).len();
        }
        let next_borrows_closing_marker = node
            .next
            .is_some_and(|next| self.needs_to_borrow_prev_closing_tag_end_marker(next));
        let mut end = node.position.end;
        if next_borrows_closing_marker {
            end -= self.closing_tag_end_marker(Some(child)).len();
        }
        let mut raw = self.src(start.min(end), end);
        // `</b\n  >`: the line break of a borrowed `>` is not part of the element.
        if node.is_html_kind(HtmlKind::Element) && next_borrows_closing_marker {
            raw = crate::ast::trim_end(raw);
        }
        concat(vec![
            self.print_opening_tag_prefix(child),
            replace_end_of_line(raw),
            self.print_closing_tag_suffix(child),
        ])
    }

    fn print_between_line(&self, prev: Option<Id>, next: Option<Id>) -> Between {
        let (Some(prev), Some(next)) = (prev, next) else {
            return Between::Nothing;
        };
        let prev_node = self.ast.get(prev);
        let next_node = self.ast.get(next);

        // `123<a\n  ><b>`: the space is printed inside the tag of the next node.
        let handled_in_next_tag = self.needs_to_borrow_next_opening_tag_start_marker(prev)
            && (self.has_prettier_ignore(next)
                || next_node.first_child.is_some()
                || self.has_no_children(next)
                || (next_node.is_html_kind(HtmlKind::Element)
                    && next_node
                        .attributes()
                        .is_some_and(|attributes| !attributes.is_empty())));
        // `<img\n  src="long"\n/>123`
        let handled_in_prev_tag = prev_node.is_html_kind(HtmlKind::Element)
            && self.has_no_close_marker(prev)
            && self.needs_to_borrow_prev_closing_tag_end_marker(next);
        if handled_in_next_tag || handled_in_prev_tag {
            return Between::Nothing;
        }

        let borrows_through_last_children = || {
            self.needs_to_borrow_prev_closing_tag_end_marker(next)
                && prev_node.last_child.is_some_and(|last| {
                    self.needs_to_borrow_parent_closing_tag_start_marker(last)
                        && self.ast.get(last).last_child.is_some_and(|last| {
                            self.needs_to_borrow_parent_closing_tag_start_marker(last)
                        })
                })
        };
        if !next_node.whitespace.is_leading_sensitive
            || self.prefer_hardline_as_leading_spaces(next)
            || borrows_through_last_children()
        {
            return Between::Hardline;
        }
        if next_node.whitespace.has_leading {
            Between::Line
        } else {
            Between::Softline
        }
    }

    pub(crate) fn print_children(&self, id: Id, args: &Args) -> Doc {
        let children = self.ast.get(id).children().unwrap_or_default();

        if self.force_break_children(id) {
            let mut parts = vec![Doc::BreakParent];
            let child_args = args.with_groups(vec![Gid::ForceBreak], vec![Gid::ForceBreak]);
            for child in children {
                let prev = self.ast.get(*child).prev;
                let between = match self.print_between_line(prev, Some(*child)) {
                    Between::Nothing => nothing(),
                    between => concat(vec![
                        between_doc(between),
                        if self.force_next_empty_line(prev) {
                            hardline()
                        } else {
                            nothing()
                        },
                    ]),
                };
                parts.push(concat(vec![between, self.print_child(*child, &child_args)]));
            }
            return concat(parts);
        }

        let leading_groups: Vec<u32> = children.iter().map(|_| self.new_group()).collect();
        let trailing_groups: Vec<u32> = children.iter().map(|_| self.new_group()).collect();

        let whitespace: Vec<WhitespaceAround> = children
            .iter()
            .enumerate()
            .map(|(index, child)| {
                let mut around = WhitespaceAround::default();
                let node = self.ast.get(*child);
                if node.is_text() {
                    return around;
                }
                let prev_between = self.print_between_line(node.prev, Some(*child));
                let next_between = self.print_between_line(Some(*child), node.next);

                if prev_between != Between::Nothing {
                    if self.force_next_empty_line(node.prev) {
                        around.leading_hardlines = 2;
                    } else if prev_between == Between::Hardline {
                        around.leading_hardlines = 1;
                    } else if self.is_text_like_node(node.prev) {
                        if node.is_liquid() && prev_between == Between::Softline {
                            around.leading_dependent.push(softline());
                        } else {
                            around.leading.push(between_doc(prev_between));
                        }
                    } else {
                        // The whitespace between two nodes is printed once: nothing here when
                        // the node before broke, otherwise a chance for this one to break.
                        around.leading.push(if_break(
                            nothing(),
                            softline(),
                            Some(trailing_groups[index - 1]),
                        ));
                    }
                }

                if next_between != Between::Nothing {
                    if self.force_next_empty_line(Some(*child)) {
                        if self.is_text_like_node(node.next) {
                            around.trailing_hardlines = 2;
                        }
                    } else if next_between == Between::Hardline {
                        if self.is_text_like_node(node.next) {
                            around.trailing_hardlines = 1;
                        }
                    } else {
                        around.trailing.push(between_doc(next_between));
                    }
                }
                around
            })
            .collect();

        let leading_space_groups = |index: usize| -> Vec<Gid> {
            if index == 0 {
                return args.leading.clone();
            }
            let prev = &whitespace[index - 1];
            let current = &whitespace[index];
            if prev.trailing_hardlines > 0 || current.leading_hardlines > 0 {
                return vec![Gid::ForceBreak];
            }
            let mut groups = Vec::new();
            if !prev.trailing.is_empty() {
                groups.push(Gid::Id(trailing_groups[index - 1]));
            }
            if !current.leading.is_empty() {
                groups.push(Gid::Id(leading_groups[index]));
            }
            if !current.leading_dependent.is_empty() {
                groups.push(Gid::Id(trailing_groups[index]));
            }
            if groups.is_empty() {
                groups.push(Gid::ForceFlat);
            }
            groups
        };
        let trailing_space_groups = |index: usize| -> Vec<Gid> {
            if index == whitespace.len() - 1 {
                return args.trailing.clone();
            }
            let current = &whitespace[index];
            let next = &whitespace[index + 1];
            if current.trailing_hardlines > 0 || next.leading_hardlines > 0 {
                return vec![Gid::ForceBreak];
            }
            if current.trailing.is_empty() {
                vec![Gid::ForceFlat]
            } else {
                vec![Gid::Id(trailing_groups[index])]
            }
        };

        let mut parts = Vec::new();
        for (index, child) in children.iter().enumerate() {
            let around = &whitespace[index];
            let child_args =
                args.with_groups(leading_space_groups(index), trailing_space_groups(index));
            let mut inner = around.leading_dependent.clone();
            inner.push(self.print_child(*child, &child_args));
            inner.extend(around.trailing.iter().cloned());
            let mut outer = around.leading.clone();
            outer.push(group_with(inner, Some(trailing_groups[index]), false));

            let mut part = vec![hardline(); around.leading_hardlines];
            part.push(group_with(outer, Some(leading_groups[index]), false));
            part.extend(vec![hardline(); around.trailing_hardlines]);
            parts.push(concat(part));
        }
        concat(parts)
    }
}

fn between_doc(between: Between) -> Doc {
    match between {
        Between::Nothing => nothing(),
        Between::Hardline => hardline(),
        Between::Line => line(),
        Between::Softline => softline(),
    }
}
