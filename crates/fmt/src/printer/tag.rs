//! The opening and closing tags of HTML elements. Where whitespace between two nodes would
//! show in the page, a line may only break inside a tag: a node then "borrows" the `>` or the
//! `<` of its neighbour.
//!
//! ```html
//! <span
//!   >text</span
//! >
//! ```

use crate::ast::{HtmlKind, Id, Kind, Markup, trim};
use crate::doc::{
    Doc, concat, hardline, indent, join, line, nothing, replace_end_of_line, softline, text,
};

use super::{Args, Gid, Printer};

impl Printer<'_> {
    fn should_not_print_closing_tag(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        !self.has_no_close_marker(id)
            && node.block_end().is_none()
            && (self.has_prettier_ignore(id)
                || node
                    .parent
                    .is_some_and(|parent| self.should_preserve_content(parent)))
    }

    /// `<p></p\n>text`: the node starts with the `>` of the node before it.
    pub(crate) fn needs_to_borrow_prev_closing_tag_end_marker(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        !node.is_liquid()
            && self.is_html_node(node.prev)
            && self.has_meaningful_lack_of_leading_whitespace(id)
    }

    /// `<p\n  ><a></a\n  ></p>`: the closing tag of the node starts with the `>` of its last
    /// child.
    pub(crate) fn needs_to_borrow_last_child_closing_tag_end_marker(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        node.is_html()
            && node.last_child.is_some_and(|last| {
                self.has_meaningful_lack_of_trailing_whitespace(last)
                    && self.is_html_node(Some(self.last_descendant(last)))
            })
            && !self.is_pre_like(id)
    }

    /// `<p>\n  text</p\n>`: the node ends with the `</p` of its parent.
    pub(crate) fn needs_to_borrow_parent_closing_tag_start_marker(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        let last_descendant = Some(self.last_descendant(id));
        self.is_html_node(node.parent)
            && node.next.is_none()
            && self.has_meaningful_lack_of_trailing_whitespace(id)
            && !node.is_liquid()
            && (self.is_text_like_node(last_descendant) || self.is_liquid_node(last_descendant))
    }

    /// `text<p\n>`: the text ends with the `<p` of the node after it.
    pub(crate) fn needs_to_borrow_next_opening_tag_start_marker(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        self.is_html_node(node.next)
            && node.is_text()
            && self.has_meaningful_lack_of_trailing_whitespace(id)
    }

    /// `<p\n  >text`: the node starts with the `>` of its parent.
    pub(crate) fn needs_to_borrow_parent_opening_tag_end_marker(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        self.is_html_node(node.parent)
            && node.prev.is_none()
            && self.has_meaningful_lack_of_leading_whitespace(id)
            && !node.is_liquid()
    }

    fn print_attributes(&self, id: Id, attr_group: u32) -> Doc {
        let node = self.ast.get(id);
        let Some(attributes) = node.attributes() else {
            return nothing();
        };
        if attributes.is_empty() {
            // `<br />`
            return text(if self.is_self_closing(id) { " " } else { "" });
        }
        let block_start = node.block_start().unwrap();
        let first = self.ast.get(attributes[0]);
        let last = self.ast.get(*attributes.last().unwrap());

        let printed: Vec<Doc> = attributes
            .iter()
            .map(|attribute| {
                // A blank line between two attributes is kept.
                let extra_newline = match self.ast.get(*attribute).prev {
                    Some(prev) if self.has_more_than_one_new_line_between(prev, *attribute) => {
                        hardline()
                    }
                    _ => nothing(),
                };
                let args = Args {
                    trailing: vec![Gid::Id(attr_group)],
                    ..Args::default()
                };
                concat(vec![extra_newline, self.print(*attribute, &args)])
            })
            .collect();

        let force_break_attr_content = self
            .src(block_start.start, last.position.end)
            .contains('\n');
        let is_single_line_link_tag = self.options.single_line_link_tags
            && self.is_void_element(id)
            && node.name() == Some("link");
        let should_not_break_attributes = (node
            .children()
            .is_some_and(|children| node.is_html_kind(HtmlKind::Element) && !children.is_empty())
            || self.is_void_element(id)
            || self.is_self_closing(id))
            && !force_break_attr_content
            && attributes.len() == 1
            && !self.ast.get(attributes[0]).is_liquid();
        let force_not_to_break_attr_content =
            is_single_line_link_tag || should_not_break_attributes;

        let between = if force_not_to_break_attr_content {
            text(" ")
        } else if self.options.single_attribute_per_line && attributes.len() > 1 {
            hardline()
        } else {
            line()
        };
        let printed_attributes = if self.is_prettier_ignore_attribute_node(node.prev) {
            replace_end_of_line(self.src(first.position.start, last.position.end))
        } else {
            join(&between, printed)
        };

        let self_closing_space = || text(if self.is_self_closing(id) { " " } else { "" });
        let trailing_inner_whitespace = if node
            .first_child
            .is_some_and(|first| self.needs_to_borrow_parent_opening_tag_end_marker(first))
            || (self.has_no_close_marker(id)
                && node.parent.is_some_and(|parent| {
                    self.needs_to_borrow_last_child_closing_tag_end_marker(parent)
                }))
            || force_not_to_break_attr_content
            || self.options.bracket_same_line
        {
            self_closing_space()
        } else if self.is_self_closing(id) {
            line()
        } else {
            softline()
        };

        concat(vec![
            indent(vec![
                if force_not_to_break_attr_content {
                    text(" ")
                } else {
                    line()
                },
                if force_break_attr_content {
                    Doc::BreakParent
                } else {
                    nothing()
                },
                printed_attributes,
            ]),
            trailing_inner_whitespace,
        ])
    }

    pub(crate) fn print_opening_tag(&self, id: Id, attr_group: u32) -> Doc {
        concat(vec![
            self.print_opening_tag_start(id),
            self.print_attributes(id, attr_group),
            if self.has_no_children(id) {
                nothing()
            } else {
                self.print_opening_tag_end(id)
            },
        ])
    }

    /// The `<name` of the node, unless the text before it took it.
    fn print_opening_tag_start(&self, id: Id) -> Doc {
        match self.ast.get(id).prev {
            Some(prev) if self.needs_to_borrow_next_opening_tag_start_marker(prev) => nothing(),
            _ => concat(vec![
                self.print_opening_tag_prefix(id),
                text(self.opening_tag_start_marker(Some(id))),
            ]),
        }
    }

    /// The `>` the node borrows from its parent or from the node before it.
    pub(crate) fn print_opening_tag_prefix(&self, id: Id) -> Doc {
        let node = self.ast.get(id);
        if self.needs_to_borrow_parent_opening_tag_end_marker(id) {
            text(self.opening_tag_end_marker(node.parent))
        } else if self.needs_to_borrow_prev_closing_tag_end_marker(id) {
            text(self.closing_tag_end_marker(node.prev))
        } else {
            nothing()
        }
    }

    /// The `>` of the opening tag, unless the first child takes it.
    fn print_opening_tag_end(&self, id: Id) -> Doc {
        match self.ast.get(id).first_child {
            Some(first) if self.needs_to_borrow_parent_opening_tag_end_marker(first) => nothing(),
            _ => text(self.opening_tag_end_marker(Some(id))),
        }
    }

    /// The `<` of a node, with its name.
    pub(crate) fn opening_tag_start_marker(&self, id: Option<Id>) -> String {
        let Some(id) = id else {
            return String::new();
        };
        let node = self.ast.get(id);
        match &node.kind {
            Kind::HtmlComment { .. } => "<!--".to_string(),
            Kind::Html {
                kind, plain_name, ..
            } => match kind {
                HtmlKind::Element | HtmlKind::SelfClosing => format!("<{}", self.compound_name(id)),
                HtmlKind::DanglingMarkerClose => format!("</{}", self.compound_name(id)),
                HtmlKind::Void | HtmlKind::Raw => {
                    format!("<{}", plain_name.as_deref().unwrap_or_default())
                }
            },
            _ => String::new(),
        }
    }

    /// The closing tag of a node. It also prints the `>` of a void element and the `/>` of a
    /// self-closing one, because those too may be borrowed.
    pub(crate) fn print_closing_tag(&self, id: Id) -> Doc {
        concat(vec![
            if self.has_no_close_marker(id) {
                nothing()
            } else {
                self.print_closing_tag_start(id)
            },
            self.print_closing_tag_end(id),
        ])
    }

    fn print_closing_tag_start(&self, id: Id) -> Doc {
        let node = self.ast.get(id);
        match node.last_child {
            Some(last) if self.needs_to_borrow_parent_closing_tag_start_marker(last) => nothing(),
            _ => concat(vec![
                // The `>` of the last child.
                if self.needs_to_borrow_last_child_closing_tag_end_marker(id) {
                    text(self.closing_tag_end_marker(node.last_child))
                } else {
                    nothing()
                },
                text(self.closing_tag_start_marker(Some(id))),
            ]),
        }
    }

    fn print_closing_tag_end(&self, id: Id) -> Doc {
        let node = self.ast.get(id);
        let borrowed = match node.next {
            Some(next) => self.needs_to_borrow_prev_closing_tag_end_marker(next),
            None => node.parent.is_some_and(|parent| {
                self.needs_to_borrow_last_child_closing_tag_end_marker(parent)
            }),
        };
        if borrowed {
            nothing()
        } else {
            concat(vec![
                text(self.closing_tag_end_marker(Some(id))),
                self.print_closing_tag_suffix(id),
            ])
        }
    }

    /// The `</name` or the `<name` the node borrows from its parent or from the node after it.
    pub(crate) fn print_closing_tag_suffix(&self, id: Id) -> Doc {
        let node = self.ast.get(id);
        if self.needs_to_borrow_parent_closing_tag_start_marker(id) {
            text(self.closing_tag_start_marker(node.parent))
        } else if self.needs_to_borrow_next_opening_tag_start_marker(id) {
            text(self.opening_tag_start_marker(node.next))
        } else {
            nothing()
        }
    }

    pub(crate) fn closing_tag_start_marker(&self, id: Option<Id>) -> String {
        let Some(id) = id else {
            return String::new();
        };
        if self.should_not_print_closing_tag(id) {
            return String::new();
        }
        match &self.ast.get(id).kind {
            Kind::Html {
                kind: HtmlKind::Element,
                ..
            } => format!("</{}", self.compound_name(id)),
            Kind::Html {
                kind: HtmlKind::Raw,
                plain_name,
                ..
            } => format!("</{}", plain_name.as_deref().unwrap_or_default()),
            _ => String::new(),
        }
    }

    pub(crate) fn closing_tag_end_marker(&self, id: Option<Id>) -> &'static str {
        let Some(id) = id else {
            return "";
        };
        if self.should_not_print_closing_tag(id) || self.is_html_dangling_marker_open(id) {
            return "";
        }
        if self.is_self_closing(id) { "/>" } else { ">" }
    }

    /// The `>` of the opening tag of a node. That of a void or a self-closing element is its
    /// closing one.
    pub(crate) fn opening_tag_end_marker(&self, id: Option<Id>) -> &'static str {
        let Some(id) = id else {
            return "";
        };
        match &self.ast.get(id).kind {
            Kind::HtmlComment { .. } => "-->",
            Kind::Html {
                kind: HtmlKind::SelfClosing | HtmlKind::Void,
                ..
            } => "",
            _ => ">",
        }
    }

    /// What an element holds, as it was written, with what its children would have borrowed.
    pub(crate) fn node_content(&self, id: Id) -> &str {
        let node = self.ast.get(id);
        let mut start = node.block_start().unwrap().end;
        if node
            .first_child
            .is_some_and(|first| self.needs_to_borrow_parent_opening_tag_end_marker(first))
        {
            start -= self.opening_tag_end_marker(Some(id)).len();
        }
        let mut end = node.block_end().unwrap().start;
        if let Some(last) = node.last_child {
            if self.needs_to_borrow_parent_closing_tag_start_marker(last) {
                end += self.closing_tag_start_marker(Some(id)).len();
            } else if self.needs_to_borrow_last_child_closing_tag_end_marker(id) {
                end -= self.closing_tag_end_marker(Some(last)).len();
            }
        }
        self.src(start, end.min(self.source.len()))
    }

    /// The name of an element, with the Liquid it may hold.
    fn compound_name(&self, id: Id) -> String {
        let Kind::Html { name, .. } = &self.ast.get(id).kind else {
            return String::new();
        };
        name.iter()
            .map(|part| match &self.ast.get(*part).kind {
                Kind::Text { value } => value.clone(),
                Kind::LiquidVariableOutput { markup, .. } => match markup {
                    Markup::Variable(variable) => format!("{{{{ {} }}}}", variable.raw_source),
                    Markup::Raw(markup) => format!("{{{{ {} }}}}", trim(markup)),
                    _ => self.node_source(*part).to_string(),
                },
                _ => self.node_source(*part).to_string(),
            })
            .collect()
    }
}
