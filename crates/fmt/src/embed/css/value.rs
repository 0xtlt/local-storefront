//! The value of a declaration: cut into words, numbers, functions and operators the way
//! `postcss-values-parser` cuts it, grouped by commas and parentheses the way Prettier groups
//! it, then printed by a port of Prettier's value printers.

use crate::doc::{
    Doc, concat, dedent, fill, group, hardline, indent, join, line, nothing, softline, text,
};

use super::super::number::print_css_number;
use super::misc::{is_next_line_empty, print_string, print_unit};

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Word {
        is_hex: bool,
        is_color: bool,
    },
    AtWord,
    Number {
        unit: String,
    },
    /// A quoted string: the value is what the quotes hold.
    Str {
        quote: char,
    },
    Operator,
    Colon,
    Comma,
    Paren,
    Comment,
    UnicodeRange,
    Func {
        group: Box<Node>,
    },
    CommaGroup {
        groups: Vec<Node>,
    },
    ParenGroup {
        open: Option<Box<Node>>,
        close: Option<Box<Node>>,
        groups: Vec<Node>,
    },
    /// What a `url()` holds, when it is not a string.
    Raw,
    /// What `selector()` holds.
    Selector,
}

#[derive(Debug, Clone, PartialEq)]
struct Node {
    kind: Kind,
    /// The text of the node: the name of a function, a number without its unit.
    value: String,
    /// Whether whitespace comes before the node.
    space_before: bool,
    /// Where the node is in the value.
    start: usize,
    end: usize,
    /// The line of the value the node starts on.
    line: usize,
    /// Whether the node is one the parser read, rather than a group made of them.
    has_source: bool,
}

impl Node {
    fn group(kind: Kind) -> Node {
        Node {
            kind,
            value: String::new(),
            space_before: false,
            start: 0,
            end: 0,
            line: 0,
            has_source: false,
        }
    }

    /// Whether the node was read from the value with nothing before it. A group never is:
    /// only what the parser reads knows what came before it.
    fn has_empty_raw_before(&self) -> bool {
        self.has_source && !self.space_before
    }

    fn is_operator(&self, operator: &str) -> bool {
        self.kind == Kind::Operator && self.value == operator
    }

    fn is_math_operator(&self) -> bool {
        self.kind == Kind::Operator && matches!(self.value.as_str(), "*" | "/" | "+" | "-" | "%")
    }

    fn is_word(&self) -> bool {
        matches!(self.kind, Kind::Word { .. } | Kind::AtWord)
    }

    fn is_plain_word(&self, value: &str) -> bool {
        matches!(self.kind, Kind::Word { .. }) && self.value == value
    }

    fn is_func(&self) -> bool {
        matches!(self.kind, Kind::Func { .. })
    }

    fn is_colon(&self) -> bool {
        self.kind == Kind::Colon
    }

    fn is_comma_group(&self) -> bool {
        matches!(self.kind, Kind::CommaGroup { .. })
    }

    /// `(…)`: a group between parentheses.
    fn is_paren_group(&self) -> bool {
        matches!(
            &self.kind,
            Kind::ParenGroup {
                open: Some(_),
                close: Some(_),
                ..
            }
        )
    }

    fn groups(&self) -> &[Node] {
        match &self.kind {
            Kind::CommaGroup { groups } | Kind::ParenGroup { groups, .. } => groups,
            _ => &[],
        }
    }

    /// `key: value`, as in `(min-width: 100px)`.
    fn is_key_value_pair(&self) -> bool {
        self.is_comma_group() && self.groups().get(1).is_some_and(Node::is_colon)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tok {
    Space,
    Colon,
    Comma,
    OpenBrace,
    CloseBrace,
    OpenParen,
    CloseParen,
    Str,
    AtWord,
    Word,
    Operator,
    Comment,
    Hash,
    UnicodeRange,
}

#[derive(Debug, Clone, Copy)]
struct Token {
    kind: Tok,
    start: usize,
    end: usize,
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | b'\t' | b'\r' | b'\x0c')
}

/// Where a word that starts at `start` ends: before the next character that cannot be part
/// of it.
fn word_end(bytes: &[u8], start: usize, number: bool) -> usize {
    let mut pos = start + 1;
    while pos < bytes.len() {
        let byte = bytes[pos];
        let ends = match byte {
            b' ' | b'\n' | b'\t' | b'\r' | b'(' | b')' | b'{' | b'}' | b'*' | b':' | b';'
            | b'@' | b'!' | b'&' | b'\'' | b'"' | b'+' | b'|' | b'~' | b'>' | b',' | b'['
            | b']' | b'\\' => true,
            b'-' => number,
            b'/' => number || bytes.get(pos + 1) == Some(&b'*'),
            _ => false,
        };
        if ends {
            break;
        }
        pos += 1;
    }
    pos
}

/// `None` when the value is one `postcss-values-parser` refuses.
fn tokenize(value: &str) -> Option<Vec<Token>> {
    let bytes = value.as_bytes();
    let mut tokens: Vec<Token> = Vec::new();
    let mut pos = 0;
    let mut paren_count = 0;
    let mut is_url_arg = false;
    while pos < bytes.len() {
        let byte = bytes[pos];
        let start = pos;
        let kind = match byte {
            _ if is_space(byte) => {
                while pos < bytes.len() && is_space(bytes[pos]) {
                    pos += 1;
                }
                Tok::Space
            }
            b':' => {
                pos += 1;
                Tok::Colon
            }
            b',' => {
                pos += 1;
                Tok::Comma
            }
            b'{' => {
                pos += 1;
                Tok::OpenBrace
            }
            b'}' => {
                pos += 1;
                Tok::CloseBrace
            }
            b'(' => {
                paren_count += 1;
                is_url_arg = !is_url_arg
                    && paren_count == 1
                    && tokens.last().is_some_and(|last| {
                        last.kind == Tok::Word && &value[last.start..last.end] == "url"
                    });
                pos += 1;
                Tok::OpenParen
            }
            b')' => {
                paren_count -= 1;
                is_url_arg = is_url_arg && paren_count > 0;
                pos += 1;
                Tok::CloseParen
            }
            b'\'' | b'"' => {
                // A quote closes the string unless an odd number of backslashes escapes it.
                let mut next = pos;
                loop {
                    next = next + 1 + value.get(next + 1..)?.find(byte as char)?;
                    let backslashes = bytes[..next]
                        .iter()
                        .rev()
                        .take_while(|b| **b == b'\\')
                        .count();
                    if backslashes % 2 == 0 {
                        break;
                    }
                }
                pos = next + 1;
                Tok::Str
            }
            b'@' => {
                pos += 1;
                while pos < bytes.len()
                    && !matches!(
                        bytes[pos],
                        b' ' | b'\n'
                            | b'\t'
                            | b'\r'
                            | b'{'
                            | b'('
                            | b')'
                            | b'\''
                            | b'"'
                            | b'\\'
                            | b';'
                            | b','
                            | b'/'
                    )
                {
                    pos += 1;
                }
                Tok::AtWord
            }
            b'\\' => {
                pos += 1;
                Tok::Word
            }
            b'+' | b'-' | b'*' => {
                if byte == b'-' && bytes.get(pos + 1) == Some(&b'-') {
                    // `--`: the start of a custom property.
                    pos += 2;
                    Tok::Word
                } else {
                    pos += 1;
                    Tok::Operator
                }
            }
            b'/' if bytes.get(pos + 1) == Some(&b'*') => {
                pos = pos + 2 + value[pos + 2..].find("*/")? + 2;
                Tok::Comment
            }
            // `//` starts a comment for this parser, which reads more than CSS. A value that
            // holds one is not formatted.
            b'/' if !is_url_arg && bytes.get(pos + 1) == Some(&b'/') => return None,
            b'#' if !bytes.get(pos + 1).is_some_and(u8::is_ascii_alphanumeric) => {
                pos += 1;
                Tok::Hash
            }
            b'u' | b'U' if bytes.get(pos + 1) == Some(&b'+') => {
                pos += 3;
                while pos < bytes.len()
                    && (bytes[pos].is_ascii_hexdigit() || matches!(bytes[pos], b'?' | b'-'))
                {
                    pos += 1;
                }
                pos = pos.min(bytes.len());
                Tok::UnicodeRange
            }
            b'/' => {
                pos += 1;
                Tok::Operator
            }
            _ => {
                let number = byte.is_ascii_digit();
                pos = word_end(bytes, pos, number);
                // The exponent of a number: `1e-10`.
                if (number || byte == b'.')
                    && pos >= 1
                    && matches!(bytes[pos - 1], b'e' | b'E')
                    && matches!(bytes.get(pos), Some(b'-' | b'+'))
                    && bytes.get(pos + 1).is_some_and(u8::is_ascii_digit)
                {
                    pos = word_end(bytes, pos, true);
                }
                // A character that is more than one byte long.
                while !value.is_char_boundary(pos) {
                    pos += 1;
                }
                Tok::Word
            }
        };
        tokens.push(Token {
            kind,
            start,
            end: pos,
        });
    }
    Some(tokens)
}

/// `^[+-]?((\d+(\.\d*)?)|(\.\d+))([eE][+-]?\d+)?`: how long the number a word starts with is.
fn number_length(word: &str) -> Option<usize> {
    let bytes = word.as_bytes();
    let mut pos = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let digits = |mut pos: usize| {
        while bytes.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1;
        }
        pos
    };
    let whole_end = digits(pos);
    if whole_end > pos {
        pos = whole_end;
        if bytes.get(pos) == Some(&b'.') {
            pos = digits(pos + 1);
        }
    } else if bytes.get(pos) == Some(&b'.') && bytes.get(pos + 1).is_some_and(u8::is_ascii_digit) {
        pos = digits(pos + 1);
    } else {
        return None;
    }
    if matches!(bytes.get(pos), Some(b'e' | b'E')) {
        let mut exponent = pos + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        if digits(exponent) > exponent {
            pos = digits(exponent);
        }
    }
    Some(pos)
}

fn is_hex_color(word: &str) -> bool {
    word.strip_prefix('#').is_some_and(|digits| {
        matches!(digits.len(), 3 | 4 | 6 | 8) && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

struct Parser<'s> {
    value: &'s str,
    tokens: Vec<Token>,
    position: usize,
    /// The containers being filled: the value, then each function entered.
    stack: Vec<Container>,
    spaces: bool,
}

struct Container {
    /// The function the container is for, or `None` for the value itself.
    func: Option<Node>,
    nodes: Vec<Node>,
    unbalanced: i32,
}

impl Parser<'_> {
    fn text(&self, token: Token) -> &str {
        &self.value[token.start..token.end]
    }

    fn line_of(&self, offset: usize) -> usize {
        self.value[..offset].matches('\n').count()
    }

    fn node(&self, kind: Kind, value: &str, start: usize, end: usize) -> Node {
        Node {
            kind,
            value: value.to_string(),
            space_before: false,
            start,
            end,
            line: self.line_of(start),
            has_source: true,
        }
    }

    fn current(&mut self) -> &mut Container {
        self.stack.last_mut().unwrap()
    }

    fn push(&mut self, mut node: Node) {
        node.space_before = std::mem::take(&mut self.spaces);
        self.current().nodes.push(node);
    }

    fn next_kind(&self) -> Option<Tok> {
        self.tokens.get(self.position + 1).map(|token| token.kind)
    }

    /// Closes the function on top of the stack: it becomes a node of the container below.
    fn close_function(&mut self) -> Option<()> {
        if self.stack.len() < 2 {
            return Some(());
        }
        let container = self.stack.pop().unwrap();
        let mut func = container.func.unwrap();
        func.kind = Kind::Func {
            group: Box::new(group_nodes(container.nodes)?),
        };
        self.stack.last_mut().unwrap().nodes.push(func);
        Some(())
    }

    fn parse(mut self) -> Option<Node> {
        while self.position < self.tokens.len() {
            let token = self.tokens[self.position];
            match token.kind {
                Tok::Space => {
                    let last = self.position == self.tokens.len() - 1;
                    if !last && self.next_kind() != Some(Tok::CloseParen) {
                        self.spaces = true;
                    } else if self.current().nodes.is_empty() && self.current().func.is_none() {
                        // Whitespace with nothing before it to attach it to.
                        return None;
                    }
                    self.position += 1;
                }
                Tok::Colon => {
                    let node = self.node(Kind::Colon, ":", token.start, token.end);
                    self.push(node);
                    self.position += 1;
                }
                Tok::Comma => {
                    let node = self.node(Kind::Comma, ",", token.start, token.end);
                    self.push(node);
                    self.position += 1;
                }
                Tok::Comment => {
                    let node = self.node(Kind::Comment, self.text(token), token.start, token.end);
                    self.push(node);
                    self.position += 1;
                }
                Tok::OpenParen => {
                    let mut unbalanced = 1;
                    for token in &self.tokens[self.position + 1..] {
                        match token.kind {
                            Tok::OpenParen => unbalanced += 1,
                            Tok::CloseParen => unbalanced -= 1,
                            _ => {}
                        }
                        if unbalanced == 0 {
                            break;
                        }
                    }
                    if unbalanced != 0 {
                        return None;
                    }
                    self.current().unbalanced += 1;
                    let node = self.node(Kind::Paren, "(", token.start, token.end);
                    self.push(node);
                    self.position += 1;
                }
                Tok::CloseParen => {
                    let node = self.node(Kind::Paren, ")", token.start, token.end);
                    self.push(node);
                    self.position += 1;
                    if self.position + 1 >= self.tokens.len() && self.current().unbalanced == 0 {
                        continue;
                    }
                    self.current().unbalanced -= 1;
                    if self.current().unbalanced < 0 {
                        return None;
                    }
                    if self.current().unbalanced == 0 {
                        self.close_function()?;
                    }
                }
                Tok::Operator => {
                    let sign = matches!(self.text(token), "+" | "-");
                    let after_operator = self
                        .current()
                        .nodes
                        .last()
                        .is_none_or(|last| last.kind == Kind::Operator);
                    if sign && after_operator && self.next_kind() == Some(Tok::Word) {
                        // A sign: `-1px`, `-webkit-box`.
                        self.word();
                    } else {
                        let node =
                            self.node(Kind::Operator, self.text(token), token.start, token.end);
                        self.push(node);
                        self.position += 1;
                    }
                }
                Tok::Str => {
                    let raw = self.text(token);
                    let quote = raw.chars().next().unwrap();
                    let node = self.node(
                        Kind::Str { quote },
                        &raw[1..raw.len() - 1],
                        token.start,
                        token.end,
                    );
                    self.push(node);
                    self.position += 1;
                }
                Tok::UnicodeRange => {
                    let node =
                        self.node(Kind::UnicodeRange, self.text(token), token.start, token.end);
                    self.push(node);
                    self.position += 1;
                }
                _ => self.word(),
            }
        }
        // Functions that were never closed.
        while self.stack.len() > 1 {
            self.close_function()?;
        }
        group_nodes(self.stack.pop().unwrap().nodes)
    }

    /// A word: the tokens that follow each other without whitespace, cut again at each `@`.
    fn word(&mut self) {
        let first = self.tokens[self.position];
        let first_text = self.text(first);
        let mut end = first.end;
        // `#`, `{` and `}` on their own stay on their own.
        let no_follow = first_text.starts_with(['{', '}'])
            || (first_text.starts_with('#')
                && !first_text[1..]
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric()));
        if !no_follow {
            while self.next_kind() == Some(Tok::Word) {
                self.position += 1;
                end = self.tokens[self.position].end;
            }
        }
        let current = self.tokens[self.position];
        let is_number = number_length(self.text(current)).is_some();
        let next_is_paren = self.next_kind() == Some(Tok::OpenParen);
        let word = &self.value[first.start..end];

        let mut cuts: Vec<usize> = vec![0];
        cuts.extend(word.match_indices('@').map(|(index, _)| index));
        cuts.dedup();
        for (index, cut) in cuts.iter().enumerate() {
            let segment_end = cuts.get(index + 1).copied().unwrap_or(word.len());
            let segment = &word[*cut..segment_end];
            let (start, end) = (first.start + cut, first.start + segment_end);
            if let Some(name) = segment.strip_prefix('@') {
                let node = self.node(Kind::AtWord, name, start, end);
                self.push(node);
            } else if is_number {
                let length = number_length(segment).unwrap_or(0);
                let unit = segment[length..].to_string();
                let node = self.node(Kind::Number { unit }, &segment[..length], start, end);
                self.push(node);
            } else if next_is_paren {
                let mut func = self.node(
                    Kind::Func {
                        group: Box::new(Node::group(Kind::Raw)),
                    },
                    segment,
                    start,
                    end,
                );
                func.space_before = std::mem::take(&mut self.spaces);
                self.stack.push(Container {
                    func: Some(func),
                    nodes: Vec::new(),
                    unbalanced: 0,
                });
            } else {
                let kind = Kind::Word {
                    is_hex: segment.len() > 1 && segment.starts_with('#'),
                    is_color: is_hex_color(segment),
                };
                let node = self.node(kind, segment, start, end);
                self.push(node);
            }
        }
        self.position += 1;
    }
}

/// Groups the nodes of a container by commas and by parentheses, then drops the groups of a
/// single node: Prettier's `parseValueNode` and `flattenGroups`.
fn group_nodes(nodes: Vec<Node>) -> Option<Node> {
    struct Level {
        open: Option<Node>,
        groups: Vec<Node>,
        comma_group: Vec<Node>,
    }
    let mut levels = vec![Level {
        open: None,
        groups: Vec::new(),
        comma_group: Vec::new(),
    }];
    let comma_group = |nodes: Vec<Node>| Node::group(Kind::CommaGroup { groups: nodes });
    let count = nodes.len();
    let trailing_comma: Vec<bool> = (0..count)
        .map(|index| {
            // `a, /* b */)`: a comma that only a comment follows is dropped.
            index + 3 == count
                && nodes[index].kind == Kind::Comma
                && nodes[index + 1].kind == Kind::Comment
                && nodes[index + 2].kind == Kind::Paren
                && nodes[index + 2].value == ")"
        })
        .collect();
    for (index, node) in nodes.into_iter().enumerate() {
        match (&node.kind, node.value.as_str()) {
            (Kind::Paren, "(") => levels.push(Level {
                open: Some(node),
                groups: Vec::new(),
                comma_group: Vec::new(),
            }),
            (Kind::Paren, _) => {
                if levels.len() == 1 {
                    return None;
                }
                let mut level = levels.pop().unwrap();
                if !level.comma_group.is_empty() {
                    level.groups.push(comma_group(level.comma_group));
                }
                let paren_group = Node::group(Kind::ParenGroup {
                    open: level.open.map(Box::new),
                    close: Some(Box::new(node)),
                    groups: level.groups,
                });
                levels.last_mut().unwrap().comma_group.push(paren_group);
            }
            (Kind::Comma, _) if trailing_comma[index] => {}
            (Kind::Comma, _) => {
                let level = levels.last_mut().unwrap();
                let group = std::mem::take(&mut level.comma_group);
                level.groups.push(comma_group(group));
            }
            _ => levels.last_mut().unwrap().comma_group.push(node),
        }
    }
    if levels.len() > 1 {
        return None;
    }
    let mut root = levels.pop().unwrap();
    if !root.comma_group.is_empty() {
        root.groups.push(comma_group(root.comma_group));
    }
    Some(flatten(Node::group(Kind::ParenGroup {
        open: None,
        close: None,
        groups: root.groups,
    })))
}

fn flatten(node: Node) -> Node {
    match node.kind {
        Kind::ParenGroup {
            open: None,
            close: None,
            mut groups,
        } if groups.len() == 1 => flatten(groups.pop().unwrap()),
        Kind::CommaGroup { mut groups } if groups.len() == 1 => flatten(groups.pop().unwrap()),
        Kind::ParenGroup {
            open,
            close,
            groups,
        } => Node {
            kind: Kind::ParenGroup {
                open,
                close,
                groups: groups.into_iter().map(flatten).collect(),
            },
            ..node
        },
        Kind::CommaGroup { groups } => Node {
            kind: Kind::CommaGroup {
                groups: groups.into_iter().map(flatten).collect(),
            },
            ..node
        },
        kind => Node { kind, ..node },
    }
}

/// What `url(…)` holds becomes its text when it is neither a string nor a function: the
/// parser cannot be trusted with a bare URL.
fn stringify_urls(node: &mut Node, value: &str) {
    match &mut node.kind {
        Kind::Func { group } => {
            if node.value == "selector"
                && let Kind::ParenGroup {
                    open: Some(open),
                    close: Some(close),
                    groups,
                } = &mut group.kind
            {
                let mut selector = Node::group(Kind::Selector);
                selector.value = value[open.end..close.start].to_string();
                *groups = vec![selector];
                return;
            }
            if node.value == "url"
                && let Kind::ParenGroup {
                    open: Some(open),
                    close: Some(close),
                    groups,
                } = &mut group.kind
            {
                let holds_string_or_function = groups
                    .iter()
                    .flat_map(|group| match &group.kind {
                        Kind::CommaGroup { groups } => groups.iter().collect::<Vec<_>>(),
                        _ => vec![group],
                    })
                    .any(|node| {
                        matches!(node.kind, Kind::Str { .. })
                            || (node.is_func() && !node.value.ends_with('\\'))
                    });
                if !holds_string_or_function {
                    let mut raw = Node::group(Kind::Raw);
                    raw.value = value[open.end..close.start].trim().to_string();
                    *groups = vec![raw];
                    return;
                }
            }
            stringify_urls(group, value);
        }
        Kind::CommaGroup { groups } | Kind::ParenGroup { groups, .. } => {
            for group in groups {
                stringify_urls(group, value);
            }
        }
        _ => {}
    }
}

/// Where the value is, which decides how some of it is laid out.
pub(super) struct Context<'a> {
    /// The style sheet, and where the value starts in it.
    pub source: &'a str,
    pub offset: usize,
    /// The property of the declaration the value is that of, in lowercase.
    pub prop: Option<&'a str>,
    /// The name of the at-rule the value is in, in lowercase.
    pub at_rule: Option<&'a str>,
    pub single_quote: bool,
}

/// What is above the node being printed.
#[derive(Clone, Copy)]
struct Above<'a> {
    parent: Option<&'a Node>,
    grandparent: Option<&'a Node>,
    /// The function the node is inside of.
    func: Option<&'a str>,
    /// How deep the node is: 0 for the value itself.
    depth: usize,
    /// The node before this one in a group separated by spaces.
    previous: Option<&'a Node>,
}

struct Printer<'a> {
    context: &'a Context<'a>,
}

fn is_possible_font_size(node: Option<&Node>) -> bool {
    match node {
        Some(node) if matches!(node.kind, Kind::Number { .. }) => true,
        Some(node) if node.is_func() => {
            let name = node.value.to_lowercase();
            matches!(name.as_str(), "var" | "calc" | "min" | "max" | "clamp")
                || name.starts_with("--")
        }
        _ => false,
    }
}

const COLOR_ADJUSTER_FUNCTIONS: [&str; 25] = [
    "red",
    "green",
    "blue",
    "alpha",
    "a",
    "rgb",
    "hue",
    "h",
    "saturation",
    "s",
    "lightness",
    "l",
    "whiteness",
    "w",
    "blackness",
    "b",
    "tint",
    "shade",
    "blend",
    "blenda",
    "contrast",
    "hsl",
    "hsla",
    "hwb",
    "hwba",
];

/// Appends to the last part of a fill, which is always a content.
fn append(parts: &mut Vec<Doc>, doc: Doc) {
    let last = parts.pop().unwrap_or_else(nothing);
    parts.push(concat(vec![last, doc]));
}

impl<'a> Printer<'a> {
    fn print(&self, node: &'a Node, above: Above<'a>) -> Doc {
        let inside = |func: Option<&'a str>| Above {
            parent: Some(node),
            grandparent: above.parent,
            func: func.or(above.func),
            depth: above.depth + 1,
            previous: None,
        };
        match &node.kind {
            Kind::CommaGroup { .. } => self.print_comma_separated_value_group(node, above),
            Kind::ParenGroup { .. } => self.print_parenthesized_value_group(node, above),
            Kind::Func { group } => {
                let keyword = matches!(node.value.to_lowercase().as_str(), "not" | "and" | "or");
                concat(vec![
                    text(node.value.as_str()),
                    text(if self.context.at_rule == Some("supports") && keyword {
                        " "
                    } else {
                        ""
                    }),
                    self.print(group, inside(Some(node.value.as_str()))),
                ])
            }
            Kind::Number { unit } => concat(vec![
                text(print_css_number(&node.value)),
                text(print_unit(unit)),
            ]),
            Kind::Word { is_hex, is_color } => {
                let wide_keyword = matches!(
                    node.value.to_lowercase().as_str(),
                    "initial" | "inherit" | "unset" | "revert"
                );
                if (*is_color && *is_hex) || wide_keyword {
                    text(node.value.to_lowercase())
                } else {
                    text(node.value.as_str())
                }
            }
            Kind::Colon => {
                let escaped = above
                    .previous
                    .is_some_and(|previous| previous.value.ends_with('\\'));
                let inside_url = above
                    .func
                    .is_some_and(|func| func.eq_ignore_ascii_case("url"));
                group(vec![
                    text(":"),
                    if escaped || inside_url {
                        nothing()
                    } else {
                        line()
                    },
                ])
            }
            Kind::Str { quote } => text(print_string(
                &format!("{quote}{}{quote}", node.value),
                self.context.single_quote,
            )),
            Kind::AtWord => text(format!("@{}", node.value)),
            Kind::Selector => super::selector::print(
                &node.value,
                &super::selector::Context {
                    single_quote: self.context.single_quote,
                    in_keyframes: false,
                },
            ),
            _ => text(node.value.as_str()),
        }
    }

    fn inside_func(&self, above: Above, name: &str) -> bool {
        above.func.is_some_and(|func| func.to_lowercase() == name)
    }

    fn print_comma_separated_value_group(&self, node: &'a Node, above: Above<'a>) -> Doc {
        let groups = node.groups();
        let prop = self.context.prop;
        let is_grid_value = above.depth == 0
            && prop.is_some_and(|prop| prop == "grid" || prop.starts_with("grid-template"));
        let inside_url = self.inside_func(above, "url");
        let inside_calc = self.inside_func(above, "calc");
        let grandparent_is_color_adjuster = above.grandparent.is_some_and(|node| {
            node.is_func() && COLOR_ADJUSTER_FUNCTIONS.contains(&node.value.to_lowercase().as_str())
        });

        let mut parts: Vec<Doc> = vec![nothing()];
        let mut inside_interpolation = false;
        let mut did_break = false;

        for (index, current) in groups.iter().enumerate() {
            let previous = index.checked_sub(1).map(|index| &groups[index]);
            let next = groups.get(index + 1);
            let child_above = Above {
                parent: Some(node),
                grandparent: above.parent,
                func: above.func,
                depth: above.depth + 1,
                previous,
            };
            append(&mut parts, self.print(current, child_above));

            let Some(next) = next else {
                continue;
            };
            if inside_url {
                if next.is_operator("+") || current.is_operator("+") {
                    append(&mut parts, text(" "));
                }
                continue;
            }
            // Tailwind's `@utility name-*`.
            if self.context.at_rule == Some("utility")
                && matches!(current.kind, Kind::Word { .. })
                && next.is_operator("*")
            {
                continue;
            }
            // A string that holds `#{` is left open for what follows it.
            if matches!(current.kind, Kind::Str { .. }) {
                match (current.value.rfind("#{"), current.value.rfind('}')) {
                    (Some(open), Some(close)) => inside_interpolation = open > close,
                    (Some(_), None) => inside_interpolation = true,
                    (None, Some(_)) => inside_interpolation = false,
                    (None, None) => {}
                }
            }
            if inside_interpolation {
                continue;
            }
            if current.is_colon() || next.is_colon() {
                continue;
            }
            if current.kind == Kind::AtWord
                && (current.value.is_empty() || current.value.ends_with('['))
            {
                continue;
            }
            if matches!(next.kind, Kind::Word { .. }) && next.value.starts_with(']') {
                continue;
            }
            if current.value == "~" {
                continue;
            }
            // An escape goes with what it escapes.
            if !matches!(current.kind, Kind::Str { .. })
                && current.value.contains('\\')
                && next.kind != Kind::Comment
            {
                continue;
            }
            if previous.is_some_and(|previous| {
                previous.value.ends_with('\\') && previous.value.matches('\\').count() == 1
            }) && current.is_operator("/")
            {
                continue;
            }
            if current.value == "\\" {
                continue;
            }
            let is_hash = |node: &Node| node.is_plain_word("#");
            let is_left_curly = |node: &Node| node.is_plain_word("{");
            let is_right_curly = |node: &Node| node.is_plain_word("}");
            if is_hash(current)
                || is_left_curly(current)
                || is_right_curly(next)
                || (is_left_curly(next) && next.has_empty_raw_before())
                || (is_right_curly(current) && next.has_empty_raw_before())
            {
                continue;
            }
            if current.value == "--" && is_hash(next) {
                continue;
            }

            let is_math_operator = current.is_math_operator();
            let is_next_math_operator = next.is_math_operator();
            if ((is_math_operator && is_hash(next))
                || (is_next_math_operator && is_right_curly(current)))
                && next.has_empty_raw_before()
            {
                continue;
            }
            // `type(<number> +)`
            if next.is_operator("+")
                && self.inside_func(above, "type")
                && next.has_empty_raw_before()
            {
                continue;
            }
            // `-fb-url(/abs/path)`
            if previous.is_none() && current.is_operator("/") {
                continue;
            }
            // Inside `calc`, a sign glued to what follows is not an operation.
            if inside_calc
                && (current.is_operator("+")
                    || next.is_operator("+")
                    || current.is_operator("-")
                    || next.is_operator("-"))
                && next.has_empty_raw_before()
            {
                continue;
            }

            let after_next = groups.get(index + 2);
            let is_sign = current.is_operator("+") || current.is_operator("-");
            let is_color_adjuster = is_sign
                && index == 0
                && (matches!(next.kind, Kind::Number { .. })
                    || matches!(next.kind, Kind::Word { is_hex: true, .. }))
                && grandparent_is_color_adjuster
                && !next.has_empty_raw_before();
            let requires_space_before_operator = after_next
                .is_some_and(|node| node.is_func() || node.is_word())
                || current.is_func()
                || current.is_word();
            let requires_space_after_operator = next.is_func()
                || next.is_word()
                || previous.is_some_and(|node| node.is_func() || node.is_word());
            // `/`, `+` and `-` between two values that are glued: `1px/2`, `a-b`.
            if !(next.is_operator("*") || current.is_operator("*"))
                && !inside_calc
                && !is_color_adjuster
                && ((next.is_operator("/") && !requires_space_before_operator)
                    || (current.is_operator("/") && !requires_space_after_operator)
                    || (next.is_operator("+") && !requires_space_before_operator)
                    || (current.is_operator("+") && !requires_space_after_operator)
                    || next.is_operator("-")
                    || current.is_operator("-"))
                && (next.has_empty_raw_before()
                    || (is_math_operator && previous.is_none_or(Node::is_math_operator)))
            {
                continue;
            }

            if self.context.at_rule == Some("namespace") {
                append(&mut parts, text(" "));
                continue;
            }
            // The lines of a grid are kept as they were written.
            if is_grid_value {
                if current.has_source && next.has_source && current.line != next.line {
                    parts.push(hardline());
                    parts.push(nothing());
                    did_break = true;
                } else {
                    append(&mut parts, text(" "));
                }
                continue;
            }
            // `font: 12px/1.5`
            if prop.is_some_and(|prop| prop == "font" || prop.starts_with("--")) {
                if next.is_operator("/")
                    && next.has_empty_raw_before()
                    && is_possible_font_size(Some(current))
                {
                    continue;
                }
                if current.is_operator("/")
                    && current.has_empty_raw_before()
                    && is_possible_font_size(previous)
                {
                    continue;
                }
            }
            if is_next_math_operator {
                append(&mut parts, text(" "));
                continue;
            }
            if next.value == "..." {
                continue;
            }
            if current.value == "with" && next.is_paren_group() {
                parts = vec![concat(vec![fill(std::mem::take(&mut parts)), text(" ")])];
                continue;
            }
            // A value that follows comments is aligned with them.
            if self.context.at_rule.is_none()
                && current.kind == Kind::Comment
                && groups[..index]
                    .iter()
                    .all(|group| group.kind == Kind::Comment)
            {
                parts.push(dedent(line()));
                parts.push(nothing());
                continue;
            }
            parts.push(line());
            parts.push(nothing());
        }

        if did_break {
            parts.insert(0, hardline());
            parts.insert(0, nothing());
        }
        // `@import url("…") screen`: the URL is not indented.
        let import_url =
            groups.len() == 2 && groups[0].value == "url" && self.context.at_rule == Some("import");
        if import_url {
            return group(fill(parts));
        }
        group(indent(fill(parts)))
    }

    /// Whether a blank line follows a node, in the style sheet.
    fn is_next_line_empty(&self, node: &Node) -> bool {
        // The end of a word that starts with a dash is off by one in Prettier, which then
        // never sees the blank line.
        if matches!(node.kind, Kind::Word { .. })
            && node.value.starts_with('-')
            && node.value != "-"
            && node.value != "--"
        {
            return false;
        }
        is_next_line_empty(self.context.source, self.context.offset + node.end)
    }

    fn print_parenthesized_value_group(&self, node: &'a Node, above: Above<'a>) -> Doc {
        let Kind::ParenGroup {
            open,
            close,
            groups,
        } = &node.kind
        else {
            return nothing();
        };
        let parent_is_func = above.parent.is_some_and(Node::is_func);
        let child_above = Above {
            parent: Some(node),
            grandparent: above.parent,
            func: above.func,
            depth: above.depth + 1,
            previous: None,
        };
        let docs: Vec<Doc> = groups
            .iter()
            .map(|group| {
                if group.kind == Kind::Raw {
                    text(group.value.as_str())
                } else {
                    self.print(group, child_above)
                }
            })
            .collect();
        let marker = |paren: &Option<Box<Node>>| {
            text(paren.as_ref().map_or("", |paren| paren.value.as_str()))
        };

        let parent_is_url = parent_is_func
            && above
                .parent
                .is_some_and(|parent| parent.value.eq_ignore_ascii_case("url"));
        let starts_with_data = groups.first().is_some_and(|first| {
            first.groups().first().is_some_and(|word| {
                matches!(word.kind, Kind::Word { .. }) && word.value.starts_with("data:")
            })
        });
        if parent_is_url && (groups.len() == 1 || starts_with_data) {
            return concat(vec![marker(open), join(&text(","), docs), marker(close)]);
        }

        if open.is_none() {
            // A list at the top of the value. In a declaration, one item per line as soon as
            // an item is made of several words.
            let top_of_declaration = above.depth == 0 && self.context.prop.is_some();
            let force_hardline = top_of_declaration
                && !self.context.prop.is_some_and(|prop| prop.starts_with("--"))
                && groups.iter().any(Node::is_comma_group);
            let last = docs.len().saturating_sub(1);
            let mut parts = Vec::new();
            for (index, doc) in docs.into_iter().enumerate() {
                if index > 0 {
                    parts.push(if force_hardline { hardline() } else { line() });
                }
                parts.push(if index == last {
                    concat(vec![doc])
                } else {
                    concat(vec![doc, text(",")])
                });
            }
            return indent(if force_hardline {
                concat(vec![hardline(), concat(parts)])
            } else {
                group(vec![
                    if top_of_declaration {
                        softline()
                    } else {
                        nothing()
                    },
                    fill(parts),
                ])
            });
        }

        let last = groups.len().saturating_sub(1);
        let mut parts = Vec::new();
        for (index, (child, mut doc)) in groups.iter().zip(docs).enumerate() {
            // The value of a pair is already indented by the parenthesis.
            if child.is_key_value_pair()
                && !matches!(child.groups()[0].kind, Kind::ParenGroup { .. })
                && child
                    .groups()
                    .get(2)
                    .is_some_and(|value| matches!(value.kind, Kind::ParenGroup { .. }))
                && matches!(&doc, Doc::Group { contents, .. } if matches!(&**contents, Doc::Indent(inner) if matches!(&**inner, Doc::Fill(_))))
            {
                doc = group(dedent(doc));
            }
            let mut part = vec![doc];
            if index == last {
                // `var(--a,)`: an empty fallback.
                let trailing_comma = above.parent.is_some_and(|parent| {
                    parent.is_func() && parent.value.eq_ignore_ascii_case("var")
                }) && close.as_ref().is_some_and(|close| {
                    child.has_source
                        && self.context.source
                            [self.context.offset + child.start..self.context.offset + close.start]
                            .trim_end()
                            .ends_with(',')
                });
                if trailing_comma {
                    part.push(text(","));
                }
            } else {
                part.push(text(","));
                // A blank line between two arguments is kept.
                if let Some(last_node) = child.groups().last().filter(|_| child.is_comma_group()) {
                    let last_node = match &last_node.kind {
                        Kind::ParenGroup {
                            close: Some(close), ..
                        } => close,
                        _ => last_node,
                    };
                    if last_node.has_source && self.is_next_line_empty(last_node) {
                        part.push(hardline());
                    }
                }
            }
            if index > 0 {
                parts.push(line());
            }
            parts.push(concat(part));
        }
        // `(key): value`
        let is_key = above.parent.is_some_and(|parent| {
            parent.is_key_value_pair()
                && parent
                    .groups()
                    .iter()
                    .position(|group| std::ptr::eq(group, node))
                    .is_some_and(|index| parent.groups().get(index + 1).is_some_and(Node::is_colon))
        });
        let doc = group(vec![
            marker(open),
            indent(vec![softline(), concat(parts)]),
            softline(),
            Doc::LineSuffixBoundary,
            marker(close),
        ]);
        if is_key { dedent(doc) } else { doc }
    }
}

/// The document of a value, or `None` when the value is not one the parser reads: it is then
/// printed as it was written.
pub(super) fn print(value: &str, context: &Context) -> Option<Doc> {
    let tokens = tokenize(value)?;
    let parser = Parser {
        value,
        tokens,
        position: 0,
        stack: vec![Container {
            func: None,
            nodes: Vec::new(),
            unbalanced: 0,
        }],
        spaces: false,
    };
    let mut root = parser.parse()?;
    stringify_urls(&mut root, value);
    let printer = Printer { context };
    Some(printer.print(
        &root,
        Above {
            parent: None,
            grandparent: None,
            func: None,
            depth: 0,
            previous: None,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{PrintOptions, print as print_doc};

    fn fmt(prop: &str, value: &str) -> String {
        let source = format!("{prop}:{value}");
        let context = Context {
            source: &source,
            offset: prop.len() + 1,
            prop: Some(prop),
            at_rule: None,
            single_quote: true,
        };
        let doc = print(value, &context).unwrap();
        print_doc(
            concat(vec![text(format!("{prop}: ")), doc]),
            &PrintOptions {
                print_width: 80,
                tab_width: 2,
                use_tabs: false,
            },
        )
    }

    #[test]
    fn prints_numbers_colors_and_strings() {
        assert_eq!(
            fmt("margin", ".5em 1.0PX +.5E3 -0.50%"),
            "margin: 0.5em 1px +0.5e3 -0.5%"
        );
        assert_eq!(fmt("color", "#FFF"), "color: #fff");
        assert_eq!(fmt("color", "RED"), "color: RED");
        assert_eq!(fmt("content", "\"a\""), "content: 'a'");
        assert_eq!(fmt("content", "\"it's\""), "content: \"it's\"");
    }

    #[test]
    fn spaces_the_arguments_of_functions() {
        assert_eq!(
            fmt("color", "rgba( 0 , 0,0,.5 )"),
            "color: rgba(0, 0, 0, 0.5)"
        );
        assert_eq!(
            fmt("width", "calc( 1px + 2px*3 )"),
            "width: calc(1px + 2px * 3)"
        );
        assert_eq!(fmt("width", "calc(1px+2px)"), "width: calc(1px+2px)");
        assert_eq!(
            fmt("width", "calc(100% - (2*var(--x,10px)))"),
            "width: calc(100% - (2 * var(--x, 10px)))"
        );
        assert_eq!(
            fmt("background", "url( \"a b.png\" )"),
            "background: url('a b.png')"
        );
        assert_eq!(
            fmt("background", "URL(Foo.PNG) No-Repeat"),
            "background: URL(Foo.PNG) No-Repeat"
        );
    }

    #[test]
    fn a_list_of_several_words_per_item_goes_one_item_per_line() {
        assert_eq!(
            fmt("transition", "opacity .3s ease,transform .3s ease"),
            "transition:\n  opacity 0.3s ease,\n  transform 0.3s ease"
        );
        assert_eq!(
            fmt("will-change", "opacity,transform"),
            "will-change: opacity, transform"
        );
        assert_eq!(fmt("transition", "opacity .3s"), "transition: opacity 0.3s");
        assert_eq!(fmt("--x", "a b,c d"), "--x: a b, c d");
    }

    #[test]
    fn leaves_glued_operators_alone() {
        assert_eq!(fmt("aspect-ratio", "16/9"), "aspect-ratio: 16/9");
        assert_eq!(fmt("font", "12px/1.5 a"), "font: 12px/1.5 a");
        assert_eq!(fmt("margin", "0 -1px"), "margin: 0 -1px");
        assert_eq!(fmt("margin", "-1px - 1px"), "margin: -1px - 1px");
        assert_eq!(fmt("grid-area", "1 / 2 / 3"), "grid-area: 1 / 2 / 3");
    }

    #[test]
    fn keeps_the_lines_of_a_grid() {
        assert_eq!(
            fmt("grid-template-areas", "\n  'a b'\n  'c d'"),
            "grid-template-areas:\n  'a b'\n  'c d'"
        );
        assert_eq!(
            fmt("grid-template-areas", "'a b' 'c d'"),
            "grid-template-areas: 'a b' 'c d'"
        );
    }
}
