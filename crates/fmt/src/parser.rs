//! Tokens → tree. A port of the document parser of Shopify's `liquid-html-parser`, in the
//! tolerant mode its Prettier plugin runs it in: what cannot be parsed becomes an error node
//! that is printed as it was written, so a source is never refused.

use crate::ast::{
    Ast, AttrKind, HtmlKind, Id, Kind, Markup, RawKind, Span, is_blank, is_space, trim, trim_end,
    trim_start,
};
use crate::liquid_doc::parse_liquid_doc;
use crate::markup::{parse_tag_markup, parse_variable};
use crate::tokenizer::{Token, TokenType, TokenizeOptions, tokenize};

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub start: usize,
    pub end: usize,
}

type Result<T> = std::result::Result<T, ParseError>;

#[derive(Clone, Copy, PartialEq)]
enum TagKind {
    Tag,
    Block,
    Raw,
    Hybrid,
}

fn tag_kind(name: &str) -> Option<TagKind> {
    Some(match name {
        "assign" | "break" | "content_for" | "continue" | "cycle" | "decrement" | "echo"
        | "include" | "increment" | "layout" | "liquid" | "render" | "sections" => TagKind::Tag,
        "block" | "capture" | "case" | "for" | "form" | "if" | "ifchanged" | "paginate"
        | "partial" | "tablerow" | "unless" => TagKind::Block,
        "comment" | "doc" | "javascript" | "raw" | "schema" | "style" | "stylesheet" => {
            TagKind::Raw
        }
        "section" => TagKind::Hybrid,
        _ => return None,
    })
}

/// The branches a block tag may have.
fn branches_of(name: &str) -> &'static [&'static str] {
    match name {
        "case" => &["when", "else"],
        "for" | "tablerow" => &["else"],
        "if" | "unless" => &["elsif", "else"],
        _ => &[],
    }
}

/// The raw tags whose body may hold Liquid.
fn parses_liquid_in_body(name: &str) -> bool {
    matches!(name, "javascript" | "style" | "stylesheet")
}

const VOID_ELEMENTS: [&str; 16] = [
    "area", "base", "br", "col", "command", "embed", "hr", "img", "input", "keygen", "link",
    "meta", "param", "source", "track", "wbr",
];
const HTML_RAW_TAGS: [&str; 3] = ["script", "style", "svg"];
const BLOCKS: [&str; 11] = [
    "if",
    "unless",
    "for",
    "case",
    "tablerow",
    "capture",
    "form",
    "paginate",
    "block",
    "ifchanged",
    "partial",
];

fn is_branch_name(name: &str) -> bool {
    matches!(name, "elsif" | "else" | "when")
}

#[derive(Clone, Copy, PartialEq)]
enum ChildFilter {
    /// Merge adjacent texts, trim them where they meet another node, drop the empty ones.
    Syntactic,
    /// Merge adjacent texts and keep their whitespace.
    Preserve,
}

/// What the delimiters of a tag say about it.
#[derive(Clone)]
struct Envelope {
    tag_name: String,
    /// Everything between the name and the closing delimiter.
    markup: Span,
    markup_offset: usize,
    markup_end: usize,
    whitespace_start: bool,
    whitespace_end: bool,
    block_start: Span,
}

/// One tag of a `{% liquid %}` tag.
#[derive(Clone)]
struct Line {
    tag_name: String,
    markup: Span,
    name_offset: usize,
    line_end: usize,
}

struct Lines {
    lines: Vec<Line>,
    index: usize,
}

/// The first word of what a tag holds.
fn first_word(inner: &str) -> &str {
    let trimmed = trim_start(inner);
    match trimmed.find(is_space) {
        Some(end) => &trimmed[..end],
        None => trim(trimmed),
    }
}

/// Matches `{%- name -%}` at `pos` for one of `names`: which one, where the tag ends, and its
/// whitespace control.
fn match_tag_at(
    source: &str,
    pos: usize,
    names: &[&str],
    word_boundary: bool,
) -> Option<(usize, usize, bool, bool)> {
    let rest = source[pos..].strip_prefix("{%")?;
    let (rest, start_dash) = match rest.strip_prefix('-') {
        Some(rest) => (rest, true),
        None => (rest, false),
    };
    let rest = trim_start(rest);
    for (index, name) in names.iter().enumerate() {
        let Some(after) = rest.strip_prefix(name) else {
            continue;
        };
        if word_boundary
            && after
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        let after = trim_start(after);
        let (after, end_dash) = match after.strip_prefix('-') {
            Some(stripped) if stripped.starts_with("%}") => (stripped, true),
            _ => (after, false),
        };
        if after.starts_with("%}") {
            let end = source.len() - after.len() + 2;
            return Some((index, end, start_dash, end_dash));
        }
    }
    None
}

struct EndTag {
    start: usize,
    end: usize,
    whitespace_start: bool,
    whitespace_end: bool,
}

/// The end tag of a raw tag, searched in the source: what a raw tag holds is not Liquid.
fn scan_for_end_tag(source: &str, search_start: usize, tag_name: &str) -> Option<EndTag> {
    let end_name = format!("end{tag_name}");
    let balanced = tag_name == "comment" || tag_name == "doc";
    let mut depth = 0;
    let mut in_raw = false;
    let mut pos = search_start;
    while let Some(found) = source[pos..].find("{%") {
        let at = pos + found;
        pos = at + 2;
        if !balanced {
            if let Some((_, end, whitespace_start, whitespace_end)) =
                match_tag_at(source, at, &[&end_name], false)
            {
                return Some(EndTag {
                    start: at,
                    end,
                    whitespace_start,
                    whitespace_end,
                });
            }
            continue;
        }
        // Comments nest, and what a `raw` holds does not count.
        let names = [end_name.as_str(), tag_name, "endraw", "raw"];
        let Some((index, end, whitespace_start, whitespace_end)) =
            match_tag_at(source, at, &names, true)
        else {
            continue;
        };
        pos = end;
        if in_raw {
            if index == 2 {
                in_raw = false;
            }
            continue;
        }
        match index {
            3 => in_raw = true,
            1 => depth += 1,
            0 => {
                if depth == 0 {
                    return Some(EndTag {
                        start: at,
                        end,
                        whitespace_start,
                        whitespace_end,
                    });
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

fn raw_kind_for_tag(name: &str, body: &str) -> RawKind {
    let holds_liquid = body.contains("{{") || body.contains("{%");
    match name {
        "javascript" => RawKind::Javascript,
        "stylesheet" | "style" => {
            if holds_liquid {
                RawKind::Text
            } else {
                RawKind::Css
            }
        }
        "schema" => RawKind::Json,
        _ => RawKind::Text,
    }
}

pub struct Parser<'s> {
    source: &'s str,
    tokens: Vec<Token>,
    p: usize,
    pub ast: Ast,
    parse_html: bool,
    allow_unclosed_html: bool,
    in_attribute_context: bool,
    in_attribute_value_context: bool,
}

/// Parses a Liquid + HTML source.
pub fn parse(source: &str) -> Ast {
    let mut parser = Parser {
        source,
        tokens: tokenize(source, TokenizeOptions::default()),
        p: 0,
        ast: Ast::default(),
        parse_html: true,
        allow_unclosed_html: true,
        in_attribute_context: false,
        in_attribute_value_context: false,
    };
    let mut children = Vec::new();
    if parser.check(TokenType::YamlFrontmatter) {
        children.push(parser.parse_yaml_frontmatter());
    }
    while !parser.is_at_end() {
        children.push(parser.parse_node());
    }
    let children = parser.filter_children(ChildFilter::Syntactic, children);
    let root = parser
        .ast
        .add(Kind::Document { children }, Span::new(0, source.len()));
    parser.ast.root = root;
    parser.ast
}

impl<'s> Parser<'s> {
    fn peek(&self) -> Token {
        self.tokens[self.p]
    }

    fn check(&self, kind: TokenType) -> bool {
        self.tokens[self.p].kind == kind
    }

    fn check_ahead(&self, kind: TokenType, ahead: usize) -> bool {
        self.tokens
            .get(self.p + ahead)
            .is_some_and(|token| token.kind == kind)
    }

    fn is_at_end(&self) -> bool {
        self.check(TokenType::EndOfInput)
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.p];
        self.p += 1;
        token
    }

    fn accept(&mut self, kind: TokenType) -> Option<Token> {
        self.check(kind).then(|| self.advance())
    }

    fn consume(&mut self, kind: TokenType) -> Result<Token> {
        let token = self.peek();
        if token.kind != kind {
            return Err(ParseError {
                message: format!("Expected {kind:?} but got {:?}", token.kind),
                start: token.start,
                end: token.end,
            });
        }
        self.p += 1;
        Ok(token)
    }

    fn slice(&self, token: Token) -> &'s str {
        &self.source[token.start..token.end]
    }

    fn error<T>(&self, message: String, start: usize, end: usize) -> Result<T> {
        Err(ParseError {
            message,
            start,
            end,
        })
    }

    /// Moves to the first token that starts at or after `offset`.
    fn seek_to_source_offset(&mut self, offset: usize) {
        self.p = self
            .tokens
            .iter()
            .position(|token| token.start >= offset)
            .unwrap_or(self.tokens.len() - 1);
    }

    /// Moves past a raw tag. A stray `{{` in what the tag holds may have made a token that
    /// runs past its end tag: the rest of the source is then tokenized again from there.
    fn reslice_tokens_from(&mut self, offset: usize) {
        let straddles = self
            .tokens
            .iter()
            .any(|token| token.start < offset && token.end > offset);
        if !straddles {
            self.seek_to_source_offset(offset);
            return;
        }
        let mut prefix: Vec<Token> = self
            .tokens
            .iter()
            .filter(|token| token.end <= offset)
            .copied()
            .collect();
        let mut open_quote = None;
        let mut inside_tag = false;
        for token in &prefix {
            match token.kind {
                TokenType::HtmlQuoteOpen => open_quote = Some(*token),
                TokenType::HtmlQuoteClose => open_quote = None,
                TokenType::HtmlTagOpen
                | TokenType::HtmlCloseTagOpen
                | TokenType::HtmlDoctypeOpen => inside_tag = true,
                TokenType::HtmlTagClose | TokenType::HtmlSelfClose => inside_tag = false,
                _ => {}
            }
        }
        let inside_quoted_attribute =
            open_quote.and_then(|token| self.source[token.start..].chars().next());
        let rebased = tokenize(
            &self.source[offset..],
            TokenizeOptions {
                skip_frontmatter: true,
                inside_quoted_attribute,
                inside_html_tag: inside_quoted_attribute.is_none() && inside_tag,
            },
        );
        self.p = prefix.len();
        prefix.extend(rebased.into_iter().map(|token| Token {
            kind: token.kind,
            start: token.start + offset,
            end: token.end + offset,
        }));
        self.tokens = prefix;
    }

    fn text_node(&mut self, start: usize, end: usize) -> Id {
        self.ast.add(
            Kind::Text {
                value: self.source[start..end].to_string(),
            },
            Span::new(start, end),
        )
    }

    fn advance_as_text(&mut self) -> Id {
        let token = self.advance();
        self.text_node(token.start, token.end)
    }

    fn is_text(&self, id: Id) -> bool {
        self.ast.get(id).is_text()
    }

    fn merge_adjacent_text_nodes(&mut self, nodes: Vec<Id>) -> Vec<Id> {
        if nodes.len() <= 1 {
            return nodes;
        }
        let mut result = Vec::with_capacity(nodes.len());
        let mut index = 0;
        while index < nodes.len() {
            if !self.is_text(nodes[index]) {
                result.push(nodes[index]);
                index += 1;
                continue;
            }
            let mut last = index;
            while last + 1 < nodes.len() && self.is_text(nodes[last + 1]) {
                last += 1;
            }
            if last == index {
                result.push(nodes[index]);
            } else {
                let start = self.ast.get(nodes[index]).position.start;
                let end = self.ast.get(nodes[last]).position.end;
                result.push(self.text_node(start, end));
            }
            index = last + 1;
        }
        result
    }

    fn filter_children(&mut self, mode: ChildFilter, children: Vec<Id>) -> Vec<Id> {
        let merged = self.merge_adjacent_text_nodes(children);
        if mode == ChildFilter::Preserve {
            return merged;
        }
        let mut result = Vec::with_capacity(merged.len());
        for id in merged {
            let node = self.ast.get(id);
            let Some(value) = node.text() else {
                result.push(id);
                continue;
            };
            // After the merge a text is always next to another kind of node, or at an edge.
            let leading = value.len() - trim_start(value).len();
            let trimmed = trim(value);
            if trimmed.is_empty() {
                continue;
            }
            let start = node.position.start + leading;
            if trimmed.len() == value.len() {
                result.push(id);
            } else {
                let end = start + trimmed.len();
                let text = self.ast.add(
                    Kind::Text {
                        value: trimmed.to_string(),
                    },
                    Span::new(start, end),
                );
                result.push(text);
            }
        }
        result
    }

    /// Parses one node. What fails to parse becomes an error node that runs to the next place
    /// where parsing can resume.
    fn parse_node(&mut self) -> Id {
        let start_token = self.peek();
        let start_position = self.p;
        match self.parse_node_strict() {
            Ok(id) => id,
            Err(_) => {
                if self.p <= start_position && !self.is_at_end() {
                    self.p += 1;
                }
                while !self.is_at_end()
                    && !matches!(
                        self.peek().kind,
                        TokenType::LiquidTagOpen
                            | TokenType::LiquidVariableOutputOpen
                            | TokenType::HtmlTagOpen
                            | TokenType::HtmlCloseTagOpen
                            | TokenType::HtmlCommentOpen
                            | TokenType::HtmlDoctypeOpen
                    )
                {
                    self.p += 1;
                }
                let end = self.peek().start;
                self.ast.add(Kind::Error, Span::new(start_token.start, end))
            }
        }
    }

    fn parse_node_strict(&mut self) -> Result<Id> {
        let token = self.peek();
        match token.kind {
            TokenType::Text => Ok(self.advance_as_text()),
            TokenType::YamlFrontmatter => Ok(self.parse_yaml_frontmatter()),
            TokenType::EndOfInput => {
                self.error("Unexpected end of input".into(), token.start, token.end)
            }
            TokenType::LiquidVariableOutputOpen => self.parse_liquid_variable_output(),
            TokenType::LiquidTagOpen => self.parse_liquid_tag(),
            TokenType::HtmlTagOpen if self.parse_html => self.parse_html_element(),
            TokenType::HtmlCloseTagOpen if self.parse_html => {
                if self.allow_unclosed_html {
                    self.parse_html_dangling_marker_close()
                } else {
                    self.parse_orphaned_html_close_tag()
                }
            }
            TokenType::HtmlCommentOpen if self.parse_html => self.parse_html_comment(),
            TokenType::HtmlDoctypeOpen if self.parse_html => self.parse_html_doctype(),
            _ => Ok(self.advance_as_text()),
        }
    }

    fn parse_yaml_frontmatter(&mut self) -> Id {
        let token = self.advance();
        let raw = self.slice(token);
        let body = match (raw.find('\n'), raw.rfind("---")) {
            (Some(first_newline), Some(last_dashes)) if last_dashes > first_newline => {
                raw[first_newline + 1..last_dashes].to_string()
            }
            _ => String::new(),
        };
        self.ast.add(
            Kind::YamlFrontmatter { body },
            Span::new(token.start, token.end),
        )
    }

    // Liquid

    fn parse_liquid_variable_output(&mut self) -> Result<Id> {
        let open = self.consume(TokenType::LiquidVariableOutputOpen)?;
        self.accept(TokenType::Text);
        let close = self.consume(TokenType::LiquidVariableOutputClose)?;
        let raw = &self.source[open.end..close.start];
        let markup = match parse_variable(raw, open.end, self.source) {
            Ok(variable) => Markup::Variable(variable),
            Err(_) => Markup::Raw(trim(raw).to_string()),
        };
        Ok(self.ast.add(
            Kind::LiquidVariableOutput {
                markup,
                whitespace_start: open.len() > 2,
                whitespace_end: close.len() > 2,
            },
            Span::new(open.start, close.end),
        ))
    }

    fn envelope_from_tokens(&self, open: Token, close: Token) -> Envelope {
        let inner = &self.source[open.end..close.start];
        let leading = inner.len() - trim_start(inner).len();
        let tag_name = first_word(inner).to_string();
        let after_name = leading + tag_name.len();
        let mut markup_offset = after_name;
        let bytes = inner.as_bytes();
        while markup_offset < bytes.len()
            && (bytes[markup_offset] == b' ' || bytes[markup_offset] == b'\t')
        {
            markup_offset += 1;
        }
        Envelope {
            tag_name,
            markup: Span::new(open.end + after_name, close.start),
            markup_offset: open.end + markup_offset,
            markup_end: close.start,
            whitespace_start: open.len() > 2,
            whitespace_end: close.len() > 2,
            block_start: Span::new(open.start, close.end),
        }
    }

    fn envelope_from_line(line: &Line) -> Envelope {
        Envelope {
            tag_name: line.tag_name.clone(),
            markup: line.markup,
            markup_offset: line.markup.start,
            markup_end: line.markup.end,
            whitespace_start: false,
            whitespace_end: false,
            block_start: Span::new(line.name_offset, line.line_end),
        }
    }

    fn markup_str(&self, envelope: &Envelope) -> &'s str {
        &self.source[envelope.markup.start..envelope.markup.end]
    }

    /// A tag: with its markup as parsed, or as written when it was not understood.
    fn make_tag(
        &mut self,
        envelope: &Envelope,
        markup: Option<Markup>,
        children: Option<Vec<Id>>,
        block_end: Option<Span>,
        delimiter_whitespace: Option<(bool, bool)>,
    ) -> Id {
        let markup =
            markup.unwrap_or_else(|| Markup::Raw(trim(self.markup_str(envelope)).to_string()));
        let end = block_end.map_or(envelope.block_start.end, |block_end| block_end.end);
        self.ast.add(
            Kind::LiquidTag {
                name: envelope.tag_name.clone(),
                markup,
                children,
                whitespace_start: envelope.whitespace_start,
                whitespace_end: envelope.whitespace_end,
                delimiter_whitespace,
                block_start: envelope.block_start,
                block_end,
            },
            Span::new(envelope.block_start.start, end),
        )
    }

    fn peek_tag_name(&self) -> Option<&'s str> {
        if !self.check(TokenType::LiquidTagOpen) {
            return None;
        }
        let inner = if self.check_ahead(TokenType::Text, 1) {
            self.slice(self.tokens[self.p + 1])
        } else {
            ""
        };
        Some(first_word(inner))
    }

    fn is_block_terminator(&self) -> bool {
        self.peek_tag_name().is_some_and(|name| {
            !name.is_empty() && (name.starts_with("end") || is_branch_name(name))
        })
    }

    fn parse_liquid_tag(&mut self) -> Result<Id> {
        let open = self.consume(TokenType::LiquidTagOpen)?;
        self.accept(TokenType::Text);
        let close = self.consume(TokenType::LiquidTagClose)?;
        let mut envelope = self.envelope_from_tokens(open, close);

        if envelope.tag_name.starts_with('#') {
            // An inline comment: everything after the `#` is its markup.
            let comment_length =
                envelope.tag_name.len() - 1 + envelope.markup.end - envelope.markup.start;
            envelope.markup.start = envelope.markup_end - comment_length;
            envelope.markup_offset = envelope.markup.start;
            envelope.tag_name = "#".to_string();
            return Ok(self.make_tag(&envelope, None, None, None, None));
        }

        if let Some(inner_name) = envelope.tag_name.strip_prefix("end")
            && tag_kind(inner_name).is_some()
        {
            return self.error(
                format!("Attempting to close LiquidTag '{inner_name}' before it was opened"),
                open.start,
                close.end,
            );
        }

        match tag_kind(&envelope.tag_name) {
            None => Ok(self.make_tag(&envelope, None, None, None, None)),
            Some(TagKind::Tag) => {
                let markup = self.parse_markup(&envelope);
                Ok(self.make_tag(&envelope, markup, None, None, None))
            }
            Some(TagKind::Block) => self.parse_block_tag(&envelope),
            Some(TagKind::Raw) => self.parse_raw_tag(&envelope, close),
            Some(TagKind::Hybrid) => self.parse_hybrid_tag(&envelope),
        }
    }

    /// The markup of a tag, or `None` when it is kept as written.
    fn parse_markup(&mut self, envelope: &Envelope) -> Option<Markup> {
        let markup = self.markup_str(envelope);
        if envelope.tag_name == "liquid" {
            let lines = parse_lines(self.source, envelope.markup);
            let mut lines = Lines { lines, index: 0 };
            let mut statements = Vec::new();
            while lines.index < lines.lines.len() {
                lines.index += 1;
                statements.push(self.parse_liquid_statement(&mut lines).ok()?);
            }
            return Some(Markup::Statements(statements));
        }
        parse_tag_markup(
            &envelope.tag_name,
            markup,
            envelope.markup.start,
            self.source,
        )
        .ok()
    }

    fn parse_block_tag(&mut self, envelope: &Envelope) -> Result<Id> {
        let markup = self.parse_markup(envelope);
        let saved = self.allow_unclosed_html;
        self.allow_unclosed_html = matches!(envelope.tag_name.as_str(), "if" | "unless" | "case");
        let branches = branches_of(&envelope.tag_name);
        let body = if branches.is_empty() {
            self.parse_block_body(envelope)
        } else {
            self.parse_branched_body(envelope, branches)
        };
        self.allow_unclosed_html = saved;
        let (children, block_end, delimiter_whitespace) = body?;
        Ok(self.make_tag(
            envelope,
            markup,
            Some(children),
            Some(block_end),
            Some(delimiter_whitespace),
        ))
    }

    fn parse_hybrid_tag(&mut self, envelope: &Envelope) -> Result<Id> {
        let markup = self.parse_markup(envelope);
        // `section` is a block only when an `endsection` follows.
        let end_name = format!("end{}", envelope.tag_name);
        let mut depth = 0;
        let mut has_end = false;
        for index in self.p..self.tokens.len().saturating_sub(1) {
            if self.tokens[index].kind != TokenType::LiquidTagOpen
                || self.tokens[index + 1].kind != TokenType::Text
            {
                continue;
            }
            let name = first_word(self.slice(self.tokens[index + 1]));
            if name == envelope.tag_name {
                depth += 1;
            } else if name == end_name {
                if depth == 0 {
                    has_end = true;
                    break;
                }
                depth -= 1;
            }
        }
        if !has_end {
            return Ok(self.make_tag(envelope, markup, None, None, None));
        }
        let (children, block_end, delimiter_whitespace) = self.parse_block_body(envelope)?;
        Ok(self.make_tag(
            envelope,
            markup,
            Some(children),
            Some(block_end),
            Some(delimiter_whitespace),
        ))
    }

    fn consume_end_tag(&mut self) -> Result<(Span, (bool, bool))> {
        let open = self.consume(TokenType::LiquidTagOpen)?;
        self.accept(TokenType::Text);
        let close = self.consume(TokenType::LiquidTagClose)?;
        Ok((
            Span::new(open.start, close.end),
            (open.len() > 2, close.len() > 2),
        ))
    }

    /// Fails when the tag that comes next closes a tag other than `parent`.
    fn check_foreign_end_tag(&self, tag_name: &str, envelope: &Envelope) -> Result<()> {
        if let Some(inner_name) = tag_name.strip_prefix("end")
            && tag_kind(inner_name).is_some()
        {
            return self.error(
                format!(
                    "Attempting to close LiquidTag '{inner_name}' before LiquidTag '{}' was closed",
                    envelope.tag_name
                ),
                envelope.block_start.start,
                envelope.block_start.end,
            );
        }
        Ok(())
    }

    /// Parses what comes next in the body of a block into `children`. `false` when nothing more
    /// can be parsed.
    fn parse_block_child(&mut self, children: &mut Vec<Id>) -> Result<bool> {
        let progress_start = self.p;
        if self.in_attribute_context && !self.in_attribute_value_context {
            children.extend(self.parse_branch_attributes()?);
        } else {
            children.push(self.parse_node());
        }
        if self.p == progress_start {
            if self.check(TokenType::LiquidTagOpen) {
                children.push(self.parse_liquid_tag()?);
                return Ok(true);
            }
            return Ok(false);
        }
        Ok(true)
    }

    fn parse_block_body(&mut self, envelope: &Envelope) -> Result<(Vec<Id>, Span, (bool, bool))> {
        let end_name = format!("end{}", envelope.tag_name);
        let mut children = Vec::new();
        while !self.is_at_end() {
            if let Some(tag_name) = self.peek_tag_name() {
                if tag_name == end_name {
                    let (position, whitespace) = self.consume_end_tag()?;
                    let children = self.filter_children(ChildFilter::Syntactic, children);
                    return Ok((children, position, whitespace));
                }
                self.check_foreign_end_tag(tag_name, envelope)?;
                if is_branch_name(tag_name) {
                    return self.error(
                        format!(
                            "Attempting to open LiquidBranch '{tag_name}' before LiquidTag '{}' was closed",
                            envelope.tag_name
                        ),
                        envelope.block_start.start,
                        envelope.block_start.end,
                    );
                }
            }
            if !self.parse_block_child(&mut children)? {
                break;
            }
        }
        // The tag is never closed.
        let children = self.filter_children(ChildFilter::Syntactic, children);
        let end = self.peek().start;
        Ok((children, Span::new(end, end), (false, false)))
    }

    fn finalize_branch(&mut self, branch: Id, children: Vec<Id>, end: usize, mode: ChildFilter) {
        let merged = self.merge_adjacent_text_nodes(children);
        let last_child_end = merged.last().map(|last| self.ast.get(*last).position.end);
        let filtered = self.filter_children(mode, merged);
        let node = &mut self.ast.nodes[branch];
        node.position.end = end.max(last_child_end.unwrap_or(end));
        if let Kind::LiquidBranch {
            children,
            block_end,
            ..
        } = &mut node.kind
        {
            *children = filtered;
            *block_end = Span::new(end, end);
        }
    }

    fn unnamed_branch(&mut self, body_start: usize) -> Id {
        self.ast.add(
            Kind::LiquidBranch {
                name: None,
                markup: Markup::Raw(String::new()),
                children: Vec::new(),
                whitespace_start: false,
                whitespace_end: false,
                block_start: Span::new(body_start, body_start),
                block_end: Span::new(body_start, body_start),
            },
            Span::new(body_start, body_start),
        )
    }

    fn named_branch(&mut self, envelope: &Envelope) -> Id {
        let raw = self.markup_str(envelope);
        let markup = match envelope.tag_name.as_str() {
            "else" => Markup::Raw(String::new()),
            name => parse_tag_markup(name, raw, envelope.markup.start, self.source)
                .unwrap_or_else(|_| Markup::Raw(trim(raw).to_string())),
        };
        self.ast.add(
            Kind::LiquidBranch {
                name: Some(envelope.tag_name.clone()),
                markup,
                children: Vec::new(),
                whitespace_start: envelope.whitespace_start,
                whitespace_end: envelope.whitespace_end,
                block_start: envelope.block_start,
                block_end: envelope.block_start,
            },
            envelope.block_start,
        )
    }

    fn parse_branched_body(
        &mut self,
        envelope: &Envelope,
        branch_names: &[&str],
    ) -> Result<(Vec<Id>, Span, (bool, bool))> {
        let end_name = format!("end{}", envelope.tag_name);
        let mut branches = Vec::new();
        let mut current = self.unnamed_branch(envelope.block_start.end);
        let mut children = Vec::new();
        let mode = if self.in_attribute_value_context {
            ChildFilter::Preserve
        } else {
            ChildFilter::Syntactic
        };
        while !self.is_at_end() {
            if let Some(tag_name) = self.peek_tag_name() {
                if tag_name == end_name {
                    let end = self.peek().start;
                    self.finalize_branch(current, std::mem::take(&mut children), end, mode);
                    branches.push(current);
                    let (position, whitespace) = self.consume_end_tag()?;
                    return Ok((branches, position, whitespace));
                }
                if branch_names.contains(&tag_name) {
                    let end = self.peek().start;
                    self.finalize_branch(current, std::mem::take(&mut children), end, mode);
                    branches.push(current);
                    let open = self.consume(TokenType::LiquidTagOpen)?;
                    self.accept(TokenType::Text);
                    let close = self.consume(TokenType::LiquidTagClose)?;
                    let branch_envelope = self.envelope_from_tokens(open, close);
                    current = self.named_branch(&branch_envelope);
                    continue;
                }
                self.check_foreign_end_tag(tag_name, envelope)?;
            }
            if !self.parse_block_child(&mut children)? {
                break;
            }
        }
        // The tag is never closed.
        let end = self.peek().start;
        self.finalize_branch(current, children, end, mode);
        branches.push(current);
        Ok((branches, Span::new(end, end), (false, false)))
    }

    fn raw_markup(&mut self, kind: RawKind, nodes: Vec<Id>, value: Span, position: Span) -> Id {
        self.ast.add(
            Kind::RawMarkup {
                kind,
                value: self.source[value.start..value.end].to_string(),
                nodes,
            },
            position,
        )
    }

    fn make_raw_tag(
        &mut self,
        envelope: &Envelope,
        body: Id,
        block_end: Span,
        delimiter_whitespace: (bool, bool),
    ) -> Id {
        self.ast.add(
            Kind::LiquidRawTag {
                name: envelope.tag_name.clone(),
                markup: trim(self.markup_str(envelope)).to_string(),
                body,
                whitespace_start: envelope.whitespace_start,
                whitespace_end: envelope.whitespace_end,
                delimiter_whitespace_start: delimiter_whitespace.0,
                delimiter_whitespace_end: delimiter_whitespace.1,
                block_start: envelope.block_start,
                block_end,
            },
            Span::new(envelope.block_start.start, block_end.end),
        )
    }

    fn parse_raw_tag(&mut self, envelope: &Envelope, close: Token) -> Result<Id> {
        let name = envelope.tag_name.as_str();
        let Some(end_tag) = scan_for_end_tag(self.source, self.peek().start, name) else {
            return self.error(
                format!("Attempting to end parsing before LiquidRawTag '{name}' was closed"),
                envelope.block_start.start,
                envelope.block_start.end,
            );
        };
        let body = Span::new(close.end, end_tag.start);
        let body_source = &self.source[body.start..body.end];
        let nodes = if name == "doc" {
            parse_liquid_doc(&mut self.ast, self.source, body)
        } else if parses_liquid_in_body(name) {
            self.parse_liquid_in_range(body.end)?
        } else if body.is_empty() {
            Vec::new()
        } else {
            let text = self.text_node(body.start, body.end);
            self.filter_children(ChildFilter::Syntactic, vec![text])
        };
        let body_id = self.raw_markup(raw_kind_for_tag(name, body_source), nodes, body, body);
        self.reslice_tokens_from(end_tag.end);
        Ok(self.make_raw_tag(
            envelope,
            body_id,
            Span::new(end_tag.start, end_tag.end),
            (end_tag.whitespace_start, end_tag.whitespace_end),
        ))
    }

    /// The Liquid and the text of the tokens that start before `body_end`: what a raw element
    /// or a raw tag holds.
    fn parse_liquid_in_range(&mut self, body_end: usize) -> Result<Vec<Id>> {
        let mut nodes = Vec::new();
        let saved_parse_html = self.parse_html;
        self.parse_html = false;
        let mut index = self.p;
        while index < self.tokens.len() && self.tokens[index].start < body_end {
            let token = self.tokens[index];
            let close_kind = match token.kind {
                TokenType::LiquidVariableOutputOpen => TokenType::LiquidVariableOutputClose,
                TokenType::LiquidTagOpen => TokenType::LiquidTagClose,
                _ => {
                    if token.kind != TokenType::EndOfInput {
                        nodes.push(self.text_node(token.start, token.end));
                    }
                    index += 1;
                    continue;
                }
            };
            let closes_within_body = self.tokens[index + 1..]
                .iter()
                .find(|token| token.kind == close_kind)
                .is_some_and(|token| token.end <= body_end);
            if !closes_within_body {
                nodes.push(self.text_node(token.start, body_end));
                while index < self.tokens.len() && self.tokens[index].start < body_end {
                    index += 1;
                }
                break;
            }
            self.p = index;
            nodes.push(if token.kind == TokenType::LiquidTagOpen {
                self.parse_liquid_tag()?
            } else {
                self.parse_liquid_variable_output()?
            });
            index = self.p;
        }
        self.parse_html = saved_parse_html;
        self.p = index.min(self.tokens.len() - 1);
        Ok(self.filter_children(ChildFilter::Syntactic, nodes))
    }

    // `{% liquid %}`

    fn parse_liquid_statement(&mut self, lines: &mut Lines) -> Result<Id> {
        let line = lines.lines[lines.index - 1].clone();
        let envelope = Self::envelope_from_line(&line);
        let name = line.tag_name.as_str();
        if name == "#" {
            // The comment is kept as written, its trailing whitespace included.
            let markup = Markup::Raw(self.markup_str(&envelope).to_string());
            return Ok(self.make_tag(&envelope, Some(markup), None, None, None));
        }
        if name.starts_with("end") {
            return self.error(
                format!("Unexpected end tag '{name}' in {{% liquid %}} block"),
                line.name_offset,
                line.line_end,
            );
        }
        match tag_kind(name) {
            None => Ok(self.make_tag(&envelope, None, None, None, None)),
            Some(TagKind::Tag) => {
                let markup = self.parse_markup(&envelope);
                Ok(self.make_tag(&envelope, markup, None, None, None))
            }
            Some(TagKind::Block) => {
                let markup = self.parse_markup(&envelope);
                let branches = branches_of(name);
                let (children, end_offset) = if branches.is_empty() {
                    self.parse_line_block_body(name, lines)?
                } else {
                    self.parse_line_branched_body(name, branches, lines)?
                };
                let block_end = Span::new(end_offset, end_offset + 3 + name.len());
                Ok(self.make_tag(
                    &envelope,
                    markup,
                    Some(children),
                    Some(block_end),
                    Some((false, false)),
                ))
            }
            Some(TagKind::Raw) => self.parse_line_raw_tag(&envelope, lines),
            Some(TagKind::Hybrid) => {
                let markup = self.parse_markup(&envelope);
                let end_name = format!("end{name}");
                let mut depth = 0;
                let mut has_end = false;
                for line in &lines.lines[lines.index..] {
                    if line.tag_name == name {
                        depth += 1;
                    } else if line.tag_name == end_name {
                        if depth == 0 {
                            has_end = true;
                            break;
                        }
                        depth -= 1;
                    }
                }
                if !has_end {
                    return Ok(self.make_tag(&envelope, markup, None, None, None));
                }
                let (children, end_offset) = self.parse_line_block_body(name, lines)?;
                let block_end = Span::new(end_offset, end_offset + 3 + name.len());
                Ok(self.make_tag(
                    &envelope,
                    markup,
                    Some(children),
                    Some(block_end),
                    Some((false, false)),
                ))
            }
        }
    }

    fn parse_line_raw_tag(&mut self, envelope: &Envelope, lines: &mut Lines) -> Result<Id> {
        let name = envelope.tag_name.as_str();
        let end_name = format!("end{name}");
        let opening_line_end = lines.lines[lines.index - 1].line_end;
        let balanced = name == "comment" || name == "doc";
        let mut end_index = None;
        let mut depth = 0;
        let mut index = lines.index;
        while index < lines.lines.len() {
            let line_name = lines.lines[index].tag_name.as_str();
            if balanced && line_name == "raw" {
                index += 1;
                while index < lines.lines.len() && lines.lines[index].tag_name != "endraw" {
                    index += 1;
                }
            } else if balanced && line_name == name {
                depth += 1;
            } else if line_name == end_name {
                if depth == 0 {
                    end_index = Some(index);
                    break;
                }
                depth -= 1;
            }
            index += 1;
        }
        let Some(end_index) = end_index else {
            return self.error(
                format!("Unclosed raw tag '{name}' in {{% liquid %}} block"),
                envelope.block_start.start,
                envelope.block_start.end,
            );
        };
        let body_end = lines.lines[end_index].name_offset;
        let body_start = (opening_line_end + 1).min(body_end);
        let nodes = if body_start < body_end {
            let text = self.text_node(body_start, body_end);
            self.filter_children(ChildFilter::Syntactic, vec![text])
        } else {
            Vec::new()
        };
        let kind = raw_kind_for_tag(name, &self.source[body_start..body_end]);
        let body = self.raw_markup(
            kind,
            nodes,
            Span::new(body_start, body_end),
            Span::new(opening_line_end, body_end),
        );
        lines.index = end_index + 1;
        Ok(self.make_raw_tag(
            envelope,
            body,
            Span::new(body_end, body_end + end_name.len()),
            (false, false),
        ))
    }

    fn parse_line_block_body(
        &mut self,
        parent: &str,
        lines: &mut Lines,
    ) -> Result<(Vec<Id>, usize)> {
        let end_name = format!("end{parent}");
        let mut children = Vec::new();
        while lines.index < lines.lines.len() {
            let line = &lines.lines[lines.index];
            let offset = line.name_offset;
            let closes = line.tag_name == end_name;
            lines.index += 1;
            if closes {
                return Ok((children, offset));
            }
            children.push(self.parse_liquid_statement(lines)?);
        }
        Ok((children, lines.lines.last().map_or(0, |line| line.line_end)))
    }

    fn parse_line_branched_body(
        &mut self,
        parent: &str,
        branch_names: &[&str],
        lines: &mut Lines,
    ) -> Result<(Vec<Id>, usize)> {
        let end_name = format!("end{parent}");
        let mut branches = Vec::new();
        let body_start = lines.lines[lines.index - 1].line_end;
        let mut current = self.unnamed_branch(body_start);
        let mut children = Vec::new();
        while lines.index < lines.lines.len() {
            let line = lines.lines[lines.index].clone();
            lines.index += 1;
            if line.tag_name == end_name {
                self.finalize_branch(current, children, line.name_offset, ChildFilter::Preserve);
                branches.push(current);
                return Ok((branches, line.name_offset));
            }
            if branch_names.contains(&line.tag_name.as_str()) {
                self.finalize_branch(
                    current,
                    std::mem::take(&mut children),
                    line.name_offset,
                    ChildFilter::Preserve,
                );
                branches.push(current);
                current = self.named_branch(&Self::envelope_from_line(&line));
                continue;
            }
            children.push(self.parse_liquid_statement(lines)?);
        }
        let end = lines.lines.last().map_or(0, |line| line.line_end);
        self.finalize_branch(current, children, end, ChildFilter::Preserve);
        branches.push(current);
        Ok((branches, end))
    }

    // HTML

    fn is_space_token(&self, token: Token) -> bool {
        self.slice(token).chars().next().is_some_and(is_space)
    }

    fn skip_space(&mut self) {
        while self.check(TokenType::Text) && self.is_space_token(self.peek()) {
            self.p += 1;
        }
    }

    /// Skips the whitespace between the name of a closing tag and its `>`.
    fn skip_blank_text(&mut self) {
        while self.check(TokenType::Text) && is_blank(self.slice(self.peek())) {
            self.p += 1;
        }
    }

    fn consume_text_segment(&mut self) -> Option<Id> {
        if !self.check(TokenType::Text) || self.is_space_token(self.peek()) {
            return None;
        }
        Some(self.advance_as_text())
    }

    fn plain_name(&self, name: &[Id]) -> Option<&str> {
        match name {
            [segment] => self.ast.get(*segment).text(),
            _ => None,
        }
    }

    fn source_of(&self, id: Id) -> &'s str {
        let position = self.ast.get(id).position;
        &self.source[position.start..position.end]
    }

    fn compound_names_match(&self, a: &[Id], b: &[Id]) -> bool {
        // A name that is wholly a Liquid tag is only known when the page renders.
        let wholly_liquid_tag = |name: &[Id]| {
            name.len() == 1
                && matches!(
                    self.ast.get(name[0]).kind,
                    Kind::LiquidTag { .. } | Kind::LiquidRawTag { .. }
                )
        };
        if wholly_liquid_tag(a) && wholly_liquid_tag(b) {
            return true;
        }
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b).all(
            |(a, b)| match (&self.ast.get(*a).kind, &self.ast.get(*b).kind) {
                (Kind::Text { value: a }, Kind::Text { value: b }) => a == b,
                (
                    Kind::LiquidVariableOutput { markup: a, .. },
                    Kind::LiquidVariableOutput { markup: b, .. },
                ) => {
                    let raw = |markup: &Markup| match markup {
                        Markup::Variable(variable) => variable.raw_source.clone(),
                        Markup::Raw(raw) => raw.clone(),
                        _ => String::new(),
                    };
                    raw(a) == raw(b)
                }
                (Kind::LiquidTag { .. }, Kind::LiquidTag { .. })
                | (Kind::LiquidRawTag { .. }, Kind::LiquidRawTag { .. }) => {
                    self.source_of(*a) == self.source_of(*b)
                }
                _ => false,
            },
        )
    }

    /// Whether the Liquid block that follows a name starts the attributes rather than
    /// continuing the name: `<div{% if a %} class="b"{% endif %}>`.
    fn liquid_tag_starts_attribute_list(&self, segments: &[Id]) -> bool {
        if !segments.iter().any(|segment| self.is_text(*segment)) {
            return false;
        }
        if !self
            .peek_tag_name()
            .is_some_and(|name| BLOCKS.contains(&name))
        {
            return false;
        }
        let mut in_tag = false;
        let mut trim_next = false;
        for index in self.p..self.tokens.len() {
            let token = self.tokens[index];
            match token.kind {
                TokenType::LiquidTagOpen => in_tag = true,
                TokenType::LiquidTagClose => {
                    in_tag = false;
                    trim_next = token.len() == 3;
                }
                TokenType::Text if !in_tag => {
                    let mut text = self.slice(token);
                    if trim_next {
                        text = trim_start(text);
                    }
                    trim_next = false;
                    if text.is_empty() {
                        continue;
                    }
                    let next_trims_left = self.tokens.get(index + 1).is_some_and(|next| {
                        matches!(
                            next.kind,
                            TokenType::LiquidVariableOutputOpen | TokenType::LiquidTagOpen
                        ) && next.len() == 3
                    });
                    if is_blank(text) && next_trims_left {
                        continue;
                    }
                    return text.chars().next().is_some_and(is_space);
                }
                TokenType::LiquidVariableOutputOpen
                | TokenType::HtmlTagClose
                | TokenType::HtmlSelfClose
                | TokenType::EndOfInput
                    if !in_tag =>
                {
                    return false;
                }
                _ => {}
            }
        }
        false
    }

    fn parse_compound_name(&mut self) -> Result<Vec<Id>> {
        let mut segments = Vec::new();
        while !self.is_at_end() {
            if self.check(TokenType::LiquidVariableOutputOpen) {
                segments.push(self.parse_liquid_variable_output()?);
                continue;
            }
            if !self.in_attribute_context
                && self.check(TokenType::LiquidTagOpen)
                && !segments.iter().any(|segment| {
                    matches!(
                        self.ast.get(*segment).kind,
                        Kind::LiquidVariableOutput { .. }
                    )
                })
                && !self.liquid_tag_starts_attribute_list(&segments)
            {
                segments.push(self.parse_liquid_tag()?);
                continue;
            }
            match self.consume_text_segment() {
                Some(segment) => segments.push(segment),
                None => break,
            }
        }
        Ok(segments)
    }

    fn parse_attributes(&mut self) -> Result<Vec<Id>> {
        let saved = self.in_attribute_context;
        self.in_attribute_context = true;
        let attributes = self.parse_attribute_list(false);
        self.in_attribute_context = saved;
        attributes
    }

    /// The attributes inside a Liquid block that is itself among attributes.
    fn parse_branch_attributes(&mut self) -> Result<Vec<Id>> {
        self.parse_attribute_list(true)
    }

    fn parse_attribute_list(&mut self, in_branch: bool) -> Result<Vec<Id>> {
        let mut attributes = Vec::new();
        while !self.is_at_end() {
            self.skip_space();
            if self.is_at_end()
                || self.check(TokenType::HtmlTagClose)
                || self.check(TokenType::HtmlSelfClose)
            {
                break;
            }
            if in_branch && self.is_block_terminator() {
                break;
            }
            if self.check(TokenType::LiquidTagOpen) {
                let saved = self.in_attribute_context;
                self.in_attribute_context = true;
                let tag = self.parse_liquid_tag();
                self.in_attribute_context = saved;
                attributes.push(tag?);
                continue;
            }
            if self.check(TokenType::LiquidVariableOutputOpen) {
                attributes.push(self.parse_liquid_variable_output()?);
                continue;
            }
            let start = self.peek().start;
            let name = self.parse_compound_name()?;
            if name.is_empty() {
                self.p += 1;
                continue;
            }
            attributes.push(self.parse_attribute_after_name(name, start)?);
        }
        Ok(attributes)
    }

    fn parse_attribute_after_name(&mut self, name: Vec<Id>, start: usize) -> Result<Id> {
        self.skip_space();
        if self.accept(TokenType::HtmlEquals).is_none() {
            let end = self.ast.get(*name.last().unwrap()).position.end;
            return Ok(self.ast.add(
                Kind::Attr {
                    kind: AttrKind::Empty,
                    name,
                    value: Vec::new(),
                    attribute_position: Span::new(end, end),
                },
                Span::new(start, end),
            ));
        }
        self.skip_space();
        let Some(quote) = self.accept(TokenType::HtmlQuoteOpen) else {
            let mut value = Vec::new();
            let mut end = self.peek().start;
            let value_start = end;
            while !self.is_at_end()
                && !self.check(TokenType::HtmlTagClose)
                && !self.check(TokenType::HtmlSelfClose)
            {
                let segment = match self.consume_text_segment() {
                    Some(segment) => segment,
                    None if self.check(TokenType::LiquidVariableOutputOpen) => {
                        self.parse_liquid_variable_output()?
                    }
                    None => break,
                };
                end = self.ast.get(segment).position.end;
                value.push(segment);
            }
            return Ok(self.ast.add(
                Kind::Attr {
                    kind: AttrKind::Unquoted,
                    name,
                    value,
                    attribute_position: Span::new(value_start, end),
                },
                Span::new(start, end),
            ));
        };
        let value = self.parse_quoted_attribute_value()?;
        let close = self.consume(TokenType::HtmlQuoteClose)?;
        let kind = if matches!(self.slice(quote), "\"" | "“" | "”") {
            AttrKind::DoubleQuoted
        } else {
            AttrKind::SingleQuoted
        };
        Ok(self.ast.add(
            Kind::Attr {
                kind,
                name,
                value,
                attribute_position: Span::new(quote.end, close.start),
            },
            Span::new(start, close.end),
        ))
    }

    fn parse_quoted_attribute_value(&mut self) -> Result<Vec<Id>> {
        let mut values = Vec::new();
        while !self.is_at_end() && !self.check(TokenType::HtmlQuoteClose) {
            if self.check(TokenType::LiquidVariableOutputOpen) {
                values.push(self.parse_liquid_variable_output()?);
            } else if self.check(TokenType::LiquidTagOpen) {
                let token = self.peek();
                if let Some(name) = self.peek_tag_name()
                    && !name.is_empty()
                    && (is_branch_name(name) || name.starts_with("end"))
                {
                    return self.error(
                        format!("Unexpected Liquid tag '{name}' inside quoted attribute value"),
                        token.start,
                        token.end,
                    );
                }
                let saved = self.in_attribute_value_context;
                self.in_attribute_value_context = true;
                let tag = self.parse_liquid_tag();
                self.in_attribute_value_context = saved;
                values.push(tag?);
            } else if self.check(TokenType::Text) {
                values.push(self.advance_as_text());
            } else {
                break;
            }
        }
        Ok(values)
    }

    #[allow(clippy::too_many_arguments)]
    fn html_node(
        &mut self,
        kind: HtmlKind,
        name: Vec<Id>,
        plain_name: Option<String>,
        attributes: Vec<Id>,
        children: Vec<Id>,
        body: Option<Id>,
        block_start: Span,
        block_end: Option<Span>,
    ) -> Id {
        let end = block_end.map_or(block_start.end, |block_end| block_end.end);
        self.ast.add(
            Kind::Html {
                kind,
                name,
                plain_name,
                attributes,
                children,
                body,
                block_start,
                block_end,
            },
            Span::new(block_start.start, end),
        )
    }

    fn parse_html_element(&mut self) -> Result<Id> {
        let open_start = self.consume(TokenType::HtmlTagOpen)?.start;
        let name = self.parse_compound_name()?;
        let attributes = self.parse_attributes()?;
        if let Some(close) = self.accept(TokenType::HtmlSelfClose) {
            let block_start = Span::new(open_start, close.end);
            return Ok(self.html_node(
                HtmlKind::SelfClosing,
                name,
                None,
                attributes,
                Vec::new(),
                None,
                block_start,
                None,
            ));
        }
        let open_end = self.consume(TokenType::HtmlTagClose)?.end;
        let block_start = Span::new(open_start, open_end);
        let plain_name = self.plain_name(&name).map(str::to_string);
        let lower_name = plain_name.as_deref().map(str::to_lowercase);
        if let Some(lower) = &lower_name {
            if VOID_ELEMENTS.contains(&lower.as_str()) {
                return Ok(self.html_node(
                    HtmlKind::Void,
                    Vec::new(),
                    plain_name,
                    attributes,
                    Vec::new(),
                    None,
                    block_start,
                    None,
                ));
            }
            if HTML_RAW_TAGS.contains(&lower.as_str()) {
                return self.parse_html_raw_body(plain_name.unwrap(), attributes, block_start);
            }
        }
        let described = plain_name.as_deref().unwrap_or("compound").to_string();

        let mut children = Vec::new();
        while !self.is_at_end()
            && !self.check(TokenType::HtmlCloseTagOpen)
            && !self.is_block_terminator()
        {
            children.push(self.parse_node());
        }
        if self.is_block_terminator() && !self.allow_unclosed_html {
            let tag_name = self.peek_tag_name().unwrap_or_default();
            return self.error(
                format!("Attempting to close LiquidTag '{tag_name}' before HtmlElement '{described}' was closed"),
                open_start,
                open_end,
            );
        }
        if self.is_at_end() || self.is_block_terminator() {
            if !self.allow_unclosed_html {
                return self.error(
                    format!(
                        "Attempting to end parsing before HtmlElement '{described}' was closed"
                    ),
                    open_start,
                    open_end,
                );
            }
            // The element is closed elsewhere: in another branch, or not at all.
            let end = children
                .last()
                .map_or(open_end, |last| self.ast.get(*last).position.end);
            let children = self.filter_children(ChildFilter::Syntactic, children);
            return Ok(self.html_node(
                HtmlKind::Element,
                name,
                None,
                attributes,
                children,
                None,
                block_start,
                Some(Span::new(end, end)),
            ));
        }

        let close_start = self.consume(TokenType::HtmlCloseTagOpen)?.start;
        let close_name = self.parse_compound_name()?;
        self.skip_blank_text();
        let close_end = self.consume(TokenType::HtmlTagClose)?.end;
        if !self.compound_names_match(&name, &close_name) {
            let closed = self.plain_name(&close_name).unwrap_or("compound");
            return self.error(
                format!("Attempting to close HtmlElement '{closed}' before HtmlElement '{described}' was closed"),
                close_start,
                close_end,
            );
        }
        let children = self.filter_children(ChildFilter::Syntactic, children);
        Ok(self.html_node(
            HtmlKind::Element,
            name,
            None,
            attributes,
            children,
            None,
            block_start,
            Some(Span::new(close_start, close_end)),
        ))
    }

    /// The token that opens the closing tag of the raw element `name`, which may be nested.
    fn scan_for_html_close_tag(&self, name: &str) -> Option<usize> {
        let lower = name.to_lowercase();
        let mut depth = 0;
        let mut index = self.p;
        while index < self.tokens.len() {
            let token = self.tokens[index];
            match token.kind {
                TokenType::HtmlTagOpen => {
                    if let Some(next) = self.tokens.get(index + 1)
                        && next.kind == TokenType::Text
                        && first_word(self.slice(*next)).to_lowercase() == lower
                    {
                        depth += 1;
                    }
                }
                TokenType::HtmlCloseTagOpen => {
                    let mut end = index + 1;
                    while end < self.tokens.len() && self.tokens[end].kind == TokenType::Text {
                        end += 1;
                    }
                    if end > index + 1 {
                        let text = &self.source[token.end..self.tokens[end - 1].end];
                        if trim(text).to_lowercase() == lower {
                            if depth == 0 {
                                return Some(index);
                            }
                            depth -= 1;
                        }
                    }
                }
                _ => {}
            }
            index += 1;
        }
        None
    }

    fn parse_html_raw_body(
        &mut self,
        name: String,
        attributes: Vec<Id>,
        block_start: Span,
    ) -> Result<Id> {
        let Some(close_index) = self.scan_for_html_close_tag(&name) else {
            return self.error(
                format!("Attempting to end parsing before HtmlRawNode '{name}' was closed"),
                block_start.start,
                block_start.end,
            );
        };
        let body = Span::new(block_start.end, self.tokens[close_index].start);
        let nodes = self.parse_liquid_in_range(body.end)?;
        let body_source = &self.source[body.start..body.end];
        let kind = match name.to_lowercase().as_str() {
            "script" => self.script_kind(&attributes),
            "style" => {
                if body_source.contains("{{") || body_source.contains("{%") {
                    RawKind::Text
                } else {
                    RawKind::Css
                }
            }
            _ => RawKind::Text,
        };
        let body_id = self.raw_markup(kind, nodes, body, body);
        self.seek_to_source_offset(body.end);
        let close_start = self.consume(TokenType::HtmlCloseTagOpen)?.start;
        while self.check(TokenType::Text) {
            self.p += 1;
        }
        let close_end = self.consume(TokenType::HtmlTagClose)?.end;
        Ok(self.html_node(
            HtmlKind::Raw,
            Vec::new(),
            Some(name),
            attributes,
            Vec::new(),
            Some(body_id),
            block_start,
            Some(Span::new(close_start, close_end)),
        ))
    }

    /// What a `<script>` holds, by its `type` attribute.
    fn script_kind(&self, attributes: &[Id]) -> RawKind {
        for attribute in attributes {
            let Kind::Attr {
                kind, name, value, ..
            } = &self.ast.get(*attribute).kind
            else {
                continue;
            };
            if *kind == AttrKind::Empty
                || !self
                    .plain_name(name)
                    .is_some_and(|name| name.eq_ignore_ascii_case("type"))
            {
                continue;
            }
            let Some(value) = self.plain_name(value) else {
                break;
            };
            return match value {
                "text/html" => RawKind::Html,
                "text/markdown" => RawKind::Markdown,
                _ if value.ends_with("json")
                    || value.ends_with("importmap")
                    || value == "speculationrules" =>
                {
                    RawKind::Json
                }
                _ => RawKind::Javascript,
            };
        }
        RawKind::Javascript
    }

    fn parse_html_comment(&mut self) -> Result<Id> {
        let open = self.consume(TokenType::HtmlCommentOpen)?;
        let Some(close) = self.source[open.end..].find("-->") else {
            return self.error(
                "Attempting to end parsing before HtmlComment '<!--' was closed".into(),
                open.start,
                open.end,
            );
        };
        let body_end = open.end + close;
        let body = trim(&self.source[open.end..body_end]).to_string();
        self.seek_to_source_offset(body_end + 3);
        Ok(self.ast.add(
            Kind::HtmlComment { body },
            Span::new(open.start, body_end + 3),
        ))
    }

    fn parse_html_doctype(&mut self) -> Result<Id> {
        let open = self.consume(TokenType::HtmlDoctypeOpen)?;
        while !self.is_at_end() && !self.check(TokenType::HtmlTagClose) {
            self.p += 1;
        }
        let close = self.consume(TokenType::HtmlTagClose)?;
        let body = trim(&self.source[open.end..close.start]);
        // `doctype html`, in any case, followed by what a legacy doctype adds.
        let legacy = body
            .get(..7)
            .filter(|word| word.eq_ignore_ascii_case("doctype"))
            .map(|_| &body[7..])
            .filter(|rest| rest.starts_with(is_space))
            .map(trim_start)
            .and_then(|rest| {
                rest.get(..4)
                    .filter(|word| word.eq_ignore_ascii_case("html"))
                    .map(|_| &rest[4..])
            });
        let legacy = match legacy {
            Some(rest) => {
                // `(.*)` stops at the first line break.
                let rest = trim_start_same_line(rest);
                let rest = rest
                    .split(['\n', '\r', '\u{2028}', '\u{2029}'])
                    .next()
                    .unwrap_or("");
                (!rest.is_empty()).then(|| rest.to_string())
            }
            None => Some(body.to_string()),
        };
        Ok(self.ast.add(
            Kind::HtmlDoctype { legacy },
            Span::new(open.start, close.end),
        ))
    }

    fn parse_orphaned_html_close_tag(&mut self) -> Result<Id> {
        let start = self.consume(TokenType::HtmlCloseTagOpen)?.start;
        let name = self.parse_compound_name()?;
        self.skip_blank_text();
        let end = self.consume(TokenType::HtmlTagClose)?.end;
        let closed = self.plain_name(&name).unwrap_or("compound");
        self.error(
            format!("Attempting to close HtmlElement '{closed}' before it was opened"),
            start,
            end,
        )
    }

    fn parse_html_dangling_marker_close(&mut self) -> Result<Id> {
        let start = self.consume(TokenType::HtmlCloseTagOpen)?.start;
        let name = self.parse_compound_name()?;
        self.skip_blank_text();
        let end = self.consume(TokenType::HtmlTagClose)?.end;
        Ok(self.html_node(
            HtmlKind::DanglingMarkerClose,
            name,
            None,
            Vec::new(),
            Vec::new(),
            None,
            Span::new(start, end),
            None,
        ))
    }
}

/// `\s*` of a pattern whose `.` then stops at a line break: the leading whitespace, line
/// breaks included.
fn trim_start_same_line(text: &str) -> &str {
    trim_start(text)
}

/// Splits what a `{% liquid %}` tag holds into its tags: one per line, where a line that ends
/// with a comma goes on with the named arguments of the next one.
fn parse_lines(source: &str, body: Span) -> Vec<Line> {
    let text = &source[body.start..body.end];
    // (start in the source, end in the source)
    let mut joined: Vec<(usize, usize)> = Vec::new();
    let mut offset = body.start;
    for raw_line in text.split('\n') {
        let start = offset;
        let end = offset + raw_line.len();
        offset = end + 1;
        let trimmed = trim_start(raw_line);
        if !trimmed.is_empty()
            && let Some(previous) = joined.last_mut()
            && trim_end(&source[previous.0..previous.1]).ends_with(',')
            && continues_arguments(trimmed)
        {
            previous.1 = end;
            continue;
        }
        joined.push((start, end));
    }

    let mut lines = Vec::new();
    for (start, end) in joined {
        let raw = &source[start..end];
        let trimmed = trim_start(raw);
        if trimmed.is_empty() {
            continue;
        }
        let name_offset = start + raw.len() - trimmed.len();
        let line_end = match raw.rfind('\n') {
            Some(last_newline) => {
                start + last_newline + 1 + trim_end(&raw[last_newline + 1..]).len()
            }
            None => start + trim_end(raw).len(),
        };
        let first_line = trimmed.split('\n').next().unwrap();
        let (tag_name, markup) = if first_line.starts_with('#') {
            let rest = &trimmed[1..];
            let skipped = usize::from(rest.starts_with([' ', '\t']));
            ("#".to_string(), Span::new(name_offset + 1 + skipped, end))
        } else {
            match first_line.find(is_space) {
                None => {
                    let name_end = name_offset + first_line.len();
                    (first_line.to_string(), Span::new(name_end, name_end))
                }
                Some(first_space) => {
                    let rest = &trimmed[first_space..];
                    let markup = trim_start(rest);
                    (
                        first_line[..first_space].to_string(),
                        Span::new(end - markup.len(), end),
                    )
                }
            }
        };
        lines.push(Line {
            tag_name,
            markup,
            name_offset,
            line_end,
        });
    }
    lines
}

/// `/^\w+\s*:/`: the line starts with a named argument.
fn continues_arguments(line: &str) -> bool {
    let word = line
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .count();
    word > 0 && trim_start(&line[word..]).starts_with(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<String> {
        let ast = parse(source);
        let children = ast.get(ast.root).children().unwrap().to_vec();
        children
            .iter()
            .map(|id| match &ast.get(*id).kind {
                Kind::Text { value } => format!("text:{value}"),
                Kind::LiquidTag { name, children, .. } => {
                    format!("tag:{name}:{}", children.as_ref().map_or(0, Vec::len))
                }
                Kind::LiquidRawTag { name, .. } => format!("raw:{name}"),
                Kind::LiquidVariableOutput { .. } => "output".to_string(),
                Kind::Html { kind, .. } => format!("html:{kind:?}"),
                Kind::Error => "error".to_string(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn parses_blocks_and_their_branches() {
        assert_eq!(
            kinds("{% if a %}x{% else %}y{% endif %} z {{ b }}"),
            vec!["tag:if:2", "text:z", "output"]
        );
    }

    #[test]
    fn a_raw_tag_ends_at_its_end_tag_whatever_it_holds() {
        assert_eq!(
            kinds("{% raw %}{{ a {% endraw %}{{ b }}"),
            vec!["raw:raw", "output"]
        );
        assert_eq!(
            kinds("{% comment %}{% comment %}a{% endcomment %}{% endcomment %}b"),
            vec!["raw:comment", "text:b"]
        );
    }

    #[test]
    fn classifies_html_elements() {
        assert_eq!(
            kinds("<div><img src=\"a\"><br/></div><script>a < b</script></p>"),
            vec!["html:Element", "html:Raw", "html:DanglingMarkerClose"]
        );
    }

    #[test]
    fn what_cannot_be_parsed_is_an_error_node() {
        assert_eq!(kinds("<a></b> x"), vec!["error"]);
        assert_eq!(kinds("{% endif %} x {{ a }}"), vec!["error", "output"]);
    }

    #[test]
    fn splits_a_liquid_tag_into_statements() {
        let source = "{% liquid\n  assign a = 1\n  if a\n    echo a\n  endif\n%}";
        let ast = parse(source);
        let tag = ast.get(ast.root).children().unwrap()[0];
        let Kind::LiquidTag {
            markup: Markup::Statements(statements),
            ..
        } = &ast.get(tag).kind
        else {
            panic!("not parsed as statements");
        };
        assert_eq!(statements.len(), 2);
    }
}
