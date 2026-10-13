//! What the printers share: questions about a node and its neighbours, and the handling of
//! text that is printed as it was written.

use crate::ast::{HtmlKind, Id, Kind, Markup, is_blank, is_space, trim, trim_end};

use super::Printer;

/// The lines of a text, without the blank lines it starts with nor the whitespace it ends
/// with.
pub(crate) fn body_lines(text: &str) -> Vec<&str> {
    let mut body = text.trim_start_matches([' ', '\t']);
    while let Some(rest) = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))
    {
        body = rest;
    }
    trim_end(body)
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

pub(crate) fn markup_lines(markup: &str) -> Vec<&str> {
    trim(markup).split('\n').collect()
}

fn leading_whitespace(line: &str) -> usize {
    line.chars().take_while(|c| is_space(*c)).count()
}

/// Removes the indentation the lines share, and the whitespace they end with. The first line
/// is left out of the count when it does not start its own line in the source.
pub(crate) fn reindent(lines: Vec<&str>, skip_first: bool) -> Vec<String> {
    let min_indent = lines
        .iter()
        .enumerate()
        .filter(|(index, line)| !(skip_first && *index == 0) && !is_blank(line))
        .map(|(_, line)| leading_whitespace(line))
        .min();
    let Some(min_indent) = min_indent else {
        return lines.into_iter().map(str::to_string).collect();
    };
    lines
        .into_iter()
        .map(|line| {
            let stripped = if leading_whitespace(line) >= min_indent {
                let offset = line
                    .char_indices()
                    .nth(min_indent)
                    .map_or(line.len(), |(offset, _)| offset);
                &line[offset..]
            } else {
                line
            };
            trim_end(stripped).to_string()
        })
        .collect()
}

pub(crate) fn has_line_break_in_range(source: &str, start: usize, end: usize) -> bool {
    start < end && source[start..end].contains('\n')
}

/// Splits a text where it has a blank line. A blank line is itself a paragraph, an empty one:
/// this is what keeps one between two paragraphs.
pub(crate) fn paragraphs(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let mut segment_start = 0;
    let mut pos = 0;
    while pos < bytes.len() {
        // A run of line breaks, each `\n` or `\r\n`.
        let mut end = pos;
        let mut count = 0;
        let mut last_start = pos;
        loop {
            let unit = if bytes.get(end) == Some(&b'\n') {
                1
            } else if bytes.get(end) == Some(&b'\r') && bytes.get(end + 1) == Some(&b'\n') {
                2
            } else {
                break;
            };
            last_start = end;
            end += unit;
            count += 1;
        }
        if count >= 2 {
            if segment_start < pos {
                result.push(&text[segment_start..pos]);
            }
            result.push(&text[last_start..end]);
            segment_start = end;
            pos = end;
        } else {
            pos = end.max(pos + 1);
        }
    }
    if segment_start < text.len() {
        result.push(&text[segment_start..]);
    }
    result
}

/// `text.split(/\s+/)`.
pub(crate) fn words(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec![""];
    }
    let mut words = Vec::new();
    let mut rest = text;
    loop {
        match rest.find(is_space) {
            Some(end) => {
                words.push(&rest[..end]);
                rest = rest[end..].trim_start_matches(is_space);
            }
            None => {
                words.push(rest);
                break;
            }
        }
    }
    words
}

/// `<!--[if IE]> ... <![endif]-->`: its opening, what it holds, and its closing.
pub(crate) fn conditional_comment(comment: &str) -> Option<(&str, &str, &str)> {
    const END: &str = "<![endif]-->";
    let start = comment.find("<!--[if")?;
    let condition_end = start + comment[start..].find(']')?;
    if !comment[condition_end..].starts_with("]>") || !comment.ends_with(END) {
        return None;
    }
    let body_start = condition_end + 2;
    let body_end = comment.len() - END.len();
    if body_start > body_end {
        return None;
    }
    Some((
        &comment[start..body_start],
        trim(&comment[body_start..body_end]),
        END,
    ))
}

/// `/^\s*prettier-ignore(?=\s|$)/m`: a line that starts with `prettier-ignore`.
fn asks_to_be_ignored(text: &str) -> bool {
    let mut line_start = 0;
    loop {
        let rest = text[line_start..].trim_start_matches(is_space);
        if let Some(after) = rest.strip_prefix("prettier-ignore")
            && after.chars().next().is_none_or(is_space)
        {
            return true;
        }
        match text[line_start..].find('\n') {
            Some(newline) => line_start += newline + 1,
            None => return false,
        }
    }
}

/// `/prettier-ignore-attribute(?:s?)(?:\s+(.+))?$/s` on the trimmed text.
fn asks_to_ignore_attributes(text: &str) -> bool {
    const MARKER: &str = "prettier-ignore-attribute";
    let text = trim(text);
    text.match_indices(MARKER).any(|(index, _)| {
        let rest = &text[index + MARKER.len()..];
        [Some(rest), rest.strip_prefix('s')]
            .into_iter()
            .flatten()
            .any(|rest| rest.is_empty() || rest.starts_with(is_space))
    })
}

impl Printer<'_> {
    /// The text of a comment, in HTML or in Liquid (`{% # ... %}`).
    fn comment_text(&self, id: Id) -> Option<&str> {
        match &self.ast.get(id).kind {
            Kind::HtmlComment { body } => Some(body),
            Kind::LiquidTag {
                name,
                markup: Markup::Raw(markup),
                ..
            } if name == "#" => Some(markup),
            _ => None,
        }
    }

    fn is_prettier_ignore_node(&self, id: Option<Id>) -> bool {
        id.and_then(|id| self.comment_text(id))
            .is_some_and(asks_to_be_ignored)
    }

    /// Whether the node is to be printed as it was written: it follows a `prettier-ignore`
    /// comment, or is one.
    pub(crate) fn has_prettier_ignore(&self, id: Id) -> bool {
        self.is_prettier_ignore_node(Some(id))
            || self.is_prettier_ignore_node(self.ast.get(id).prev)
    }

    pub(crate) fn is_prettier_ignore_attribute_node(&self, id: Option<Id>) -> bool {
        id.and_then(|id| self.comment_text(id))
            .is_some_and(asks_to_ignore_attributes)
    }

    pub(crate) fn is_html_node(&self, id: Option<Id>) -> bool {
        id.is_some_and(|id| self.ast.get(id).is_html())
    }

    pub(crate) fn is_liquid_node(&self, id: Option<Id>) -> bool {
        id.is_some_and(|id| self.ast.get(id).is_liquid())
    }

    pub(crate) fn is_text_like_node(&self, id: Option<Id>) -> bool {
        id.is_some_and(|id| self.ast.get(id).is_text())
    }

    fn html_kind(&self, id: Id) -> Option<HtmlKind> {
        self.ast.get(id).html_kind()
    }

    /// An element whose closing tag is elsewhere: `{% if a %}<div>{% endif %}`.
    pub(crate) fn is_html_dangling_marker_open(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        node.is_html_kind(HtmlKind::Element) && node.block_end().is_some_and(|end| end.is_empty())
    }

    pub(crate) fn is_self_closing(&self, id: Id) -> bool {
        self.html_kind(id) == Some(HtmlKind::SelfClosing)
    }

    pub(crate) fn is_void_element(&self, id: Id) -> bool {
        self.html_kind(id) == Some(HtmlKind::Void)
    }

    pub(crate) fn has_no_children(&self, id: Id) -> bool {
        matches!(
            self.html_kind(id),
            Some(HtmlKind::SelfClosing | HtmlKind::Void | HtmlKind::DanglingMarkerClose)
        ) || self.ast.get(id).is_comment()
    }

    pub(crate) fn has_no_close_marker(&self, id: Id) -> bool {
        self.has_no_children(id) || self.is_html_dangling_marker_open(id)
    }

    fn has_non_text_child(&self, id: Id) -> bool {
        self.ast
            .get(id)
            .children()
            .is_some_and(|children| children.iter().any(|child| !self.ast.get(*child).is_text()))
    }

    pub(crate) fn should_preserve_content(&self, id: Id) -> bool {
        self.is_pre_like(id)
    }

    /// Whether the element `id` is named with one of `names`, and nothing else.
    fn is_tag_name_included(&self, names: &[&str], id: Id) -> bool {
        let node = self.ast.get(id);
        matches!(
            node.html_kind(),
            Some(HtmlKind::Element | HtmlKind::SelfClosing | HtmlKind::DanglingMarkerClose)
        ) && self
            .ast
            .plain_compound_name(id)
            .is_some_and(|name| names.contains(&name))
    }

    fn is_element(&self, id: Id) -> bool {
        self.html_kind(id) == Some(HtmlKind::Element)
    }

    /// Whether a blank line follows the node in the source, which is then kept.
    pub(crate) fn force_next_empty_line(&self, id: Option<Id>) -> bool {
        let Some(id) = id else {
            return false;
        };
        let node = self.ast.get(id);
        let Some(next) = node.next else {
            return false;
        };
        let Some(first) = self.source[node.position.end..].find('\n') else {
            return false;
        };
        let after_first = node.position.end + first + 1;
        self.source[after_first..]
            .find('\n')
            .is_some_and(|second| after_first + second < self.ast.get(next).position.start)
    }

    /// Whether what an element holds goes on its own lines.
    pub(crate) fn force_break_content(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        if self.force_break_children(id) {
            return true;
        }
        if self.is_element(id) {
            let children = node.children().unwrap_or_default();
            if !children.is_empty()
                && (self.is_tag_name_included(&["body", "script", "style"], id)
                    || children.iter().any(|child| self.has_non_text_child(*child)))
            {
                return true;
            }
        }
        match (node.first_child, node.last_child) {
            (Some(first), Some(last)) if first == last => {
                let child = self.ast.get(first);
                !child.is_text()
                    && self.has_leading_line_break(first)
                    && (!child.whitespace.is_trailing_sensitive
                        || self.has_trailing_line_break(last))
            }
            _ => false,
        }
    }

    /// Whether each child of an element goes on its own line.
    pub(crate) fn force_break_children(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        self.is_element(id)
            && node.children().is_some_and(|children| !children.is_empty())
            && (self.is_tag_name_included(&["html", "head", "ul", "ol", "select"], id)
                || (node.css_display.starts_with("table") && node.css_display != "table-cell"))
    }

    fn prefer_hardline_as_surrounding_spaces(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        match &node.kind {
            Kind::HtmlComment { .. } => true,
            Kind::Html {
                kind: HtmlKind::Element,
                ..
            } => self.is_tag_name_included(&["script", "select"], id),
            Kind::LiquidTag { children, .. } => {
                if self.is_text_like_node(node.prev) || self.is_text_like_node(node.next) {
                    return false;
                }
                children
                    .as_ref()
                    .is_some_and(|children| !children.is_empty())
            }
            _ => false,
        }
    }

    pub(crate) fn prefer_hardline_as_leading_spaces(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        self.prefer_hardline_as_surrounding_spaces(id)
            || (node.is_liquid() && self.is_liquid_node(node.prev))
            || node
                .prev
                .is_some_and(|prev| self.prefer_hardline_as_trailing_spaces(prev))
            || self.has_surrounding_line_break(id)
    }

    fn prefer_hardline_as_trailing_spaces(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        self.prefer_hardline_as_surrounding_spaces(id)
            || (node.is_liquid()
                && (self.is_liquid_node(node.next) || self.is_html_node(node.next)))
            || (self.is_element(id) && self.is_tag_name_included(&["br"], id))
            || self.has_surrounding_line_break(id)
    }

    pub(crate) fn has_meaningful_lack_of_leading_whitespace(&self, id: Id) -> bool {
        let whitespace = &self.ast.get(id).whitespace;
        whitespace.is_leading_sensitive && !whitespace.has_leading
    }

    pub(crate) fn has_meaningful_lack_of_trailing_whitespace(&self, id: Id) -> bool {
        let whitespace = &self.ast.get(id).whitespace;
        whitespace.is_trailing_sensitive && !whitespace.has_trailing
    }

    pub(crate) fn has_meaningful_lack_of_dangling_whitespace(&self, id: Id) -> bool {
        let whitespace = &self.ast.get(id).whitespace;
        whitespace.is_dangling_sensitive && !whitespace.has_dangling
    }

    fn has_surrounding_line_break(&self, id: Id) -> bool {
        self.has_leading_line_break(id) && self.has_trailing_line_break(id)
    }

    fn has_leading_line_break(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        let Some(parent) = self.ast.parent(id) else {
            return false;
        };
        let start = match node.prev {
            Some(prev) => self.ast.get(prev).position.end,
            None => parent
                .block_start()
                .map_or(parent.position.start, |block_start| block_start.end),
        };
        node.whitespace.has_leading
            && has_line_break_in_range(self.source, start, node.position.start)
    }

    fn has_trailing_line_break(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        let Some(parent) = self.ast.parent(id) else {
            return false;
        };
        let end = match node.next {
            Some(next) => self.ast.get(next).position.start,
            None => parent
                .block_end()
                .map_or(parent.position.end, |block_end| block_end.start),
        };
        node.whitespace.has_trailing && has_line_break_in_range(self.source, node.position.end, end)
    }

    pub(crate) fn last_descendant(&self, id: Id) -> Id {
        match self.ast.get(id).last_child {
            Some(last) => self.last_descendant(last),
            None => id,
        }
    }

    /// Whether more than one line break separates two nodes.
    pub(crate) fn has_more_than_one_new_line_between(&self, prev: Id, next: Id) -> bool {
        let start = self.ast.get(prev).position.end;
        let end = self.ast.get(next).position.start;
        start < end && self.source[start..end].matches('\n').count() > 1
    }

    /// Whether a tag holds tags that hold something.
    pub(crate) fn is_deeply_nested(&self, id: Id) -> bool {
        let Some(children) = self.ast.get(id).children() else {
            return false;
        };
        if self.ast.is_branched_tag(id) {
            return children.iter().any(|child| self.is_deeply_nested(*child));
        }
        children.iter().any(|child| {
            self.ast
                .get(*child)
                .children()
                .is_some_and(|children| !children.is_empty())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reindents_from_the_least_indented_line() {
        assert_eq!(
            reindent(vec!["    a", "      b  ", "", "    c"], false),
            vec!["a", "  b", "", "c"]
        );
        // The first line does not count when it follows something on its line.
        assert_eq!(
            reindent(vec!["a", "    b", "  c"], true),
            vec!["a", "  b", "c"]
        );
    }

    #[test]
    fn a_blank_line_is_a_paragraph() {
        assert_eq!(paragraphs("a b\n\n\nc"), vec!["a b", "\n", "c"]);
        assert_eq!(paragraphs("a\nb"), vec!["a\nb"]);
        assert_eq!(words("a  b\n c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn finds_the_comments_that_ask_to_be_left_alone() {
        assert!(asks_to_be_ignored("prettier-ignore"));
        assert!(asks_to_be_ignored("why\n  prettier-ignore because"));
        assert!(!asks_to_be_ignored("prettier-ignore-attribute"));
        assert!(asks_to_ignore_attributes("prettier-ignore-attribute"));
        assert!(asks_to_ignore_attributes(
            "prettier-ignore-attributes class"
        ));
        assert!(!asks_to_ignore_attributes("prettier-ignore"));
    }

    #[test]
    fn splits_a_conditional_comment() {
        assert_eq!(
            conditional_comment("<!--[if lt IE 9]> <p>old</p> <![endif]-->"),
            Some(("<!--[if lt IE 9]>", "<p>old</p>", "<![endif]-->"))
        );
        assert_eq!(conditional_comment("<!-- a -->"), None);
    }
}
