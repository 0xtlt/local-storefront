//! What the printer needs to know about every node before it prints one: its parent and
//! siblings, how the browser displays it, and whether the whitespace around it can be changed
//! without changing the page. A port of the `preprocess` step of Shopify's Prettier plugin.

use crate::ast::{Ast, HtmlKind, Id, Kind, Markup, Whitespace, is_space};
use crate::options::{CaptureWhitespaceSensitivity, HtmlWhitespaceSensitivity, Options};

/// The CSS `display` of the HTML elements, from the HTML specification's default style sheet
/// and Prettier's additions to it.
fn css_display_of_tag(name: &str) -> Option<&'static str> {
    Some(match name {
        "area" | "base" | "basefont" | "datalist" | "head" | "link" | "meta" | "noembed"
        | "noframes" | "rp" | "style" | "title" => "none",
        "param" | "script" | "source" | "track" | "html" | "body" | "address" | "blockquote"
        | "center" | "div" | "figure" | "figcaption" | "footer" | "form" | "header" | "hr"
        | "legend" | "listing" | "main" | "p" | "plaintext" | "pre" | "xmp" | "article"
        | "aside" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "hgroup" | "nav" | "section"
        | "dir" | "dd" | "dl" | "dt" | "ol" | "ul" | "fieldset" | "details" | "summary"
        | "dialog" | "option" | "optgroup" => "block",
        "template" => "inline",
        "slot" => "contents",
        "ruby" => "ruby",
        "rt" => "ruby-text",
        "li" => "list-item",
        "table" => "table",
        "caption" => "table-caption",
        "colgroup" => "table-column-group",
        "col" => "table-column",
        "thead" => "table-header-group",
        "tbody" => "table-row-group",
        "tfoot" => "table-footer-group",
        "tr" => "table-row",
        "td" | "th" => "table-cell",
        "button" | "meter" | "progress" | "object" | "video" | "audio" | "select" => "inline-block",
        _ => return None,
    })
}

fn css_display_of_liquid_tag(name: &str) -> &'static str {
    match name {
        "tablerow" | "form" | "section" => "block",
        "break" | "continue" | "comment" | "layout" | "style" | "assign" => "none",
        _ => "inline",
    }
}

fn css_white_space_of_tag(name: &str) -> Option<&'static str> {
    Some(match name {
        "listing" | "plaintext" | "pre" | "xmp" => "pre",
        "nobr" => "nowrap",
        "table" => "initial",
        "textarea" => "pre-wrap",
        _ => return None,
    })
}

/// `<!-- display: block -->` and `<!-- white-space: pre -->`: the value a comment gives the
/// node that follows it.
fn css_value_from_comment<'a>(body: &'a str, properties: &[&str]) -> Option<&'a str> {
    let body = body.trim_start_matches(is_space);
    let rest = properties
        .iter()
        .find_map(|property| body.strip_prefix(property))?
        .strip_prefix(':')?;
    let value = rest.trim_matches(is_space);
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_lowercase())).then_some(value)
}

fn is_block_like(display: &str) -> bool {
    display == "block" || display == "list-item" || display.starts_with("table")
}

/// Whether whitespace matters just inside the node, on either side.
fn is_inner_sensitive(display: &str) -> bool {
    !is_block_like(display) && display != "inline-block"
}

struct Preprocessor<'a> {
    ast: &'a mut Ast,
    source: &'a str,
    options: &'a Options,
}

impl Preprocessor<'_> {
    fn link(&mut self, id: Id) {
        let lists: Vec<Vec<Id>> = self
            .ast
            .node_lists(id)
            .into_iter()
            .map(<[Id]>::to_vec)
            .collect();
        for list in &lists {
            for (index, child) in list.iter().enumerate() {
                let node = &mut self.ast.nodes[*child];
                node.parent = Some(id);
                node.prev = index.checked_sub(1).map(|prev| list[prev]);
                node.next = list.get(index + 1).copied();
            }
        }
        if let Some(body) = self.ast.single_child(id) {
            self.ast.nodes[body].parent = Some(id);
        }
        let children = self.ast.get(id).children().map(<[Id]>::to_vec);
        let node = &mut self.ast.nodes[id];
        node.first_child = children
            .as_ref()
            .and_then(|children| children.first().copied());
        node.last_child = children
            .as_ref()
            .and_then(|children| children.last().copied());
        for child in lists.into_iter().flatten().chain(self.ast.single_child(id)) {
            self.link(child);
        }
    }

    fn all_nodes(&self) -> Vec<Id> {
        let mut found = Vec::new();
        let mut stack = vec![self.ast.root];
        while let Some(id) = stack.pop() {
            found.push(id);
            for list in self.ast.node_lists(id) {
                stack.extend_from_slice(list);
            }
            stack.extend(self.ast.single_child(id));
        }
        found
    }

    /// What a comment before the node says of one of its CSS properties.
    fn css_from_previous_comment(&self, id: Id, properties: &[&str]) -> Option<String> {
        let prev = self.ast.prev(id)?;
        let body = match &prev.kind {
            Kind::HtmlComment { body } => body.as_str(),
            Kind::LiquidTag {
                name,
                markup: Markup::Raw(markup),
                ..
            } if name == "#" => markup.as_str(),
            _ => return None,
        };
        css_value_from_comment(body, properties).map(str::to_string)
    }

    fn html_element_name(&self, id: Id) -> Option<&str> {
        match &self.ast.get(id).kind {
            Kind::Html {
                kind: HtmlKind::Void | HtmlKind::Raw,
                plain_name,
                ..
            } => plain_name.as_deref(),
            Kind::Html { .. } => self.ast.plain_compound_name(id),
            _ => None,
        }
    }

    fn css_display(&self, id: Id) -> String {
        if let Some(display) = self.css_from_previous_comment(id, &["display"]) {
            return display;
        }
        let by_sensitivity = |default: &str| match self.options.html_whitespace_sensitivity {
            HtmlWhitespaceSensitivity::Strict => "inline".to_string(),
            HtmlWhitespaceSensitivity::Ignore => "block".to_string(),
            HtmlWhitespaceSensitivity::Css => default.to_string(),
        };
        match &self.ast.get(id).kind {
            Kind::Html { .. } => by_sensitivity(
                self.html_element_name(id)
                    .and_then(css_display_of_tag)
                    .unwrap_or("inline"),
            ),
            Kind::LiquidTag { name, .. } | Kind::LiquidRawTag { name, .. } => {
                by_sensitivity(css_display_of_liquid_tag(name))
            }
            Kind::HtmlDoctype { .. }
            | Kind::HtmlComment { .. }
            | Kind::Document { .. }
            | Kind::YamlFrontmatter { .. } => "block".to_string(),
            _ => "inline".to_string(),
        }
    }

    fn css_white_space(&self, id: Id) -> String {
        if let Some(white_space) =
            self.css_from_previous_comment(id, &["white-space", "whitespace"])
        {
            return white_space;
        }
        match &self.ast.get(id).kind {
            Kind::Html { .. } => self
                .html_element_name(id)
                .and_then(css_white_space_of_tag)
                .unwrap_or("normal"),
            Kind::RawMarkup { .. } | Kind::YamlFrontmatter { .. } | Kind::LiquidRawTag { .. } => {
                "pre"
            }
            Kind::LiquidTag { name, .. } if name == "capture" => {
                match self.options.capture_whitespace_sensitivity {
                    CaptureWhitespaceSensitivity::Strict => "pre",
                    CaptureWhitespaceSensitivity::Ignore => "normal",
                }
            }
            _ => "normal",
        }
        .to_string()
    }

    fn display(&self, id: Id) -> &str {
        &self.ast.get(id).css_display
    }

    fn is_pre_like(&self, id: Id) -> bool {
        self.ast.get(id).css_whitespace.starts_with("pre")
    }

    fn is_script_like(&self, id: Id) -> bool {
        self.ast.get(id).is_html_kind(HtmlKind::Raw)
    }

    fn is_whitespace_at(&self, offset: usize) -> bool {
        self.source[offset.min(self.source.len())..]
            .chars()
            .next()
            .is_some_and(is_space)
    }

    fn is_whitespace_before(&self, offset: usize) -> bool {
        self.source[..offset]
            .chars()
            .next_back()
            .is_some_and(is_space)
    }

    fn is_html_element_without_close_tag(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        node.is_html_kind(HtmlKind::Element) && node.block_end().is_some_and(|end| end.is_empty())
    }

    /// `{{ a -}}`, `{% a -%}`, `{% endif -%}`.
    fn is_trimming_outer_right(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        match &node.kind {
            Kind::LiquidRawTag { .. } | Kind::LiquidTag { .. } => node
                .delimiter_whitespace()
                .map_or(node.whitespace_end(), |(_, end)| end),
            Kind::LiquidVariableOutput { .. } => node.whitespace_end(),
            _ => false,
        }
    }

    /// `{{- a }}`, `{%- a %}`, `{%- else %}`.
    fn is_trimming_outer_left(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        node.is_liquid() && node.whitespace_start()
    }

    /// `{% form a -%}`, `{% else -%}`.
    fn is_trimming_inner_left(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        match &node.kind {
            Kind::LiquidRawTag { .. } | Kind::LiquidTag { .. } => {
                node.delimiter_whitespace().is_some() && node.whitespace_end()
            }
            Kind::LiquidBranch { .. } => {
                let Some(parent) = node
                    .parent
                    .filter(|parent| self.ast.get(*parent).is_liquid_tag())
                else {
                    return false;
                };
                if node.prev.is_none() {
                    return self.is_trimming_inner_left(parent);
                }
                node.whitespace_end()
            }
            _ => false,
        }
    }

    /// `{%- endform %}`, and a branch that the next one trims.
    fn is_trimming_inner_right(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        match &node.kind {
            Kind::LiquidRawTag { .. } | Kind::LiquidTag { .. } => {
                node.delimiter_whitespace().is_some_and(|(start, _)| start)
            }
            Kind::LiquidBranch { .. } => {
                let Some(parent) = node
                    .parent
                    .filter(|parent| self.ast.get(*parent).is_liquid_tag())
                else {
                    return false;
                };
                match node.next {
                    None => self.is_trimming_inner_right(parent),
                    Some(next) => self.is_trimming_outer_left(next),
                }
            }
            _ => false,
        }
    }

    /// An output inside the name or the value of an attribute.
    fn is_output_inside_attribute(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        matches!(node.kind, Kind::LiquidVariableOutput { .. })
            && node
                .parent
                .is_some_and(|parent| self.ast.is_attribute_node(parent))
    }

    fn is_leading_whitespace_sensitive(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        if let Kind::LiquidBranch { name, .. } = &node.kind {
            let parent_is_sensitive = is_inner_sensitive(self.display(node.parent.unwrap()));
            if name.is_none() {
                return parent_is_sensitive
                    && node
                        .first_child
                        .is_none_or(|first| self.is_leading_whitespace_sensitive(first));
            }
            return parent_is_sensitive;
        }
        if self.is_output_inside_attribute(id) {
            return true;
        }
        if self.is_trimming_outer_left(id) {
            return false;
        }
        if node
            .prev
            .is_some_and(|prev| self.is_trimming_outer_right(prev))
        {
            return false;
        }
        let Some(parent) = node.parent.filter(|parent| self.display(*parent) != "none") else {
            return false;
        };
        if self.is_pre_like(parent) {
            return true;
        }
        if self.is_script_like(id) {
            return false;
        }
        if node.prev.is_none()
            && (matches!(self.ast.get(parent).kind, Kind::Document { .. })
                || self.is_pre_like(id)
                || self.is_script_like(parent)
                || !is_inner_sensitive(self.display(parent))
                || self.is_trimming_inner_left(parent))
        {
            return false;
        }
        if node
            .prev
            .is_some_and(|prev| is_block_like(self.display(prev)))
        {
            return false;
        }
        !is_block_like(self.display(id))
    }

    fn is_trailing_whitespace_sensitive(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        if node.is_branch() {
            let parent = node.parent.unwrap();
            let last_child_is_sensitive = node
                .last_child
                .is_none_or(|last| self.is_trailing_whitespace_sensitive(last));
            if self.ast.get(parent).last_child == Some(id) {
                return is_inner_sensitive(self.display(parent)) && last_child_is_sensitive;
            }
            return last_child_is_sensitive;
        }
        if self.is_html_element_without_close_tag(id) {
            return match node.last_child {
                None => is_inner_sensitive(self.display(id)),
                Some(last) => {
                    let display = self.display(id);
                    (is_block_like(display) || display == "inline" || display == "inline-block")
                        && self.is_trailing_whitespace_sensitive(last)
                }
            };
        }
        if self.is_trimming_outer_right(id) {
            return false;
        }
        if self.is_output_inside_attribute(id) {
            return true;
        }
        if node
            .next
            .is_some_and(|next| self.is_trimming_outer_left(next))
        {
            return false;
        }
        let Some(parent) = node.parent.filter(|parent| self.display(*parent) != "none") else {
            return false;
        };
        if self.is_pre_like(parent) {
            return true;
        }
        if self.is_script_like(id) {
            return false;
        }
        if node.is_html() && node.name() == Some("br") {
            return false;
        }
        if node.next.is_none()
            && (matches!(self.ast.get(parent).kind, Kind::Document { .. })
                || self.is_pre_like(id)
                || self.is_script_like(parent)
                || (!self.is_html_element_without_close_tag(parent)
                    && !is_inner_sensitive(self.display(parent)))
                || self.is_trimming_inner_right(parent)
                || self.ast.is_attribute_node(id))
        {
            return false;
        }
        if node
            .next
            .is_some_and(|next| is_block_like(self.display(next)))
        {
            return false;
        }
        !is_block_like(self.display(id))
    }

    /// Whether an empty node holds whitespace.
    fn has_dangling_whitespace(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        match &node.kind {
            Kind::Document { children } => children.is_empty() && !self.source.is_empty(),
            Kind::LiquidTag {
                children: Some(children),
                ..
            } if self.ast.is_branched_tag(id) && children.len() == 1 => {
                self.has_dangling_whitespace(children[0])
            }
            _ => match node.children() {
                Some([]) => node
                    .block_start()
                    .is_some_and(|block_start| self.is_whitespace_at(block_start.end)),
                _ => false,
            },
        }
    }

    fn has_leading_whitespace(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        if node.is_branch() && node.prev.is_none() {
            return match node.first_child {
                Some(first) => self.has_leading_whitespace(first),
                None => self.has_dangling_whitespace(id),
            };
        }
        self.is_whitespace_before(node.position.start)
    }

    fn has_trailing_whitespace(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        if node.is_branch() || self.is_html_element_without_close_tag(id) {
            return match node.last_child {
                Some(last) => self.has_trailing_whitespace(last),
                None => self.has_dangling_whitespace(id),
            };
        }
        self.is_whitespace_at(node.position.end)
    }

    /// Whether the indentation of what the node holds is part of what it renders.
    fn is_indentation_sensitive(&self, id: Id) -> bool {
        let node = self.ast.get(id);
        match &node.kind {
            Kind::Html { plain_name, .. } => plain_name
                .as_deref()
                .and_then(css_white_space_of_tag)
                .is_some_and(|white_space| white_space.starts_with("pre")),
            _ => node.is_liquid() && matches!(node.name(), Some("capture" | "raw")),
        }
    }

    fn whitespace(&self, id: Id) -> Whitespace {
        let node = self.ast.get(id);
        let indentation_sensitive = self.is_indentation_sensitive(id);
        Whitespace {
            is_dangling_sensitive: is_inner_sensitive(self.display(id))
                && !self.is_script_like(id)
                && !self.is_trimming_inner_left(id)
                && !self.is_trimming_inner_right(id),
            is_whitespace_sensitive: indentation_sensitive,
            is_indentation_sensitive: indentation_sensitive,
            is_leading_sensitive: self.is_leading_whitespace_sensitive(id)
                && node
                    .prev
                    .is_none_or(|prev| self.is_trailing_whitespace_sensitive(prev)),
            is_trailing_sensitive: self.is_trailing_whitespace_sensitive(id)
                && node
                    .next
                    .is_none_or(|next| self.is_leading_whitespace_sensitive(next)),
            has_leading: self.has_leading_whitespace(id),
            has_trailing: self.has_trailing_whitespace(id),
            has_dangling: self.has_dangling_whitespace(id),
        }
    }
}

/// Whether the whitespace of a node is ever looked at: not that of the parts of a `{% doc %}`
/// nor of what could not be parsed.
fn has_whitespace(kind: &Kind) -> bool {
    !matches!(
        kind,
        Kind::DocParam { .. }
            | Kind::DocExample { .. }
            | Kind::DocDescription { .. }
            | Kind::DocPrompt { .. }
            | Kind::Error
    )
}

pub fn preprocess(ast: &mut Ast, source: &str, options: &Options) {
    let root = ast.root;
    let mut preprocessor = Preprocessor {
        ast,
        source,
        options,
    };
    preprocessor.link(root);
    let nodes = preprocessor.all_nodes();
    for id in &nodes {
        let display = preprocessor.css_display(*id);
        let white_space = preprocessor.css_white_space(*id);
        let node = &mut preprocessor.ast.nodes[*id];
        node.css_display = display;
        node.css_whitespace = white_space;
    }
    for id in &nodes {
        if has_whitespace(&preprocessor.ast.get(*id).kind) {
            let whitespace = preprocessor.whitespace(*id);
            preprocessor.ast.nodes[*id].whitespace = whitespace;
        }
    }
}
