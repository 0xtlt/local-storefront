//! The tree a Liquid + HTML source is parsed into: the one Shopify's `liquid-html-parser`
//! builds, with what its Prettier plugin adds to every node before printing (parent, siblings,
//! CSS display, whitespace sensitivity).

pub type Id = usize;

/// A range of the source, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlKind {
    Element,
    DanglingMarkerClose,
    SelfClosing,
    Void,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrKind {
    SingleQuoted,
    DoubleQuoted,
    Unquoted,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawKind {
    Css,
    Html,
    Javascript,
    Json,
    Markdown,
    Text,
}

/// An expression of a Liquid tag or output. These carry no whitespace of their own, so they
/// live outside the arena.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    String {
        value: String,
        span: Span,
    },
    Number {
        value: String,
        span: Span,
    },
    Literal {
        keyword: String,
        span: Span,
    },
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        span: Span,
    },
    Lookup {
        name: Option<String>,
        lookups: Vec<Expr>,
        span: Span,
    },
    Comparison {
        comparator: String,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Logical {
        relation: String,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    /// A condition used as a value: `{{ a > b }}`.
    Boolean {
        condition: Box<Expr>,
        span: Span,
    },
    /// `[a, b]`, as the value of an argument of `block` and `section`.
    Array {
        elements: Vec<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::String { span, .. }
            | Expr::Number { span, .. }
            | Expr::Literal { span, .. }
            | Expr::Range { span, .. }
            | Expr::Lookup { span, .. }
            | Expr::Comparison { span, .. }
            | Expr::Logical { span, .. }
            | Expr::Boolean { span, .. }
            | Expr::Array { span, .. } => *span,
        }
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Expr::String { .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Argument {
    Positional(Expr),
    Named {
        name: String,
        value: Expr,
        span: Span,
    },
}

impl Argument {
    pub fn span(&self) -> Span {
        match self {
            Argument::Positional(expr) => expr.span(),
            Argument::Named { span, .. } => *span,
        }
    }

    pub fn is_named(&self) -> bool {
        matches!(self, Argument::Named { .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub name: String,
    pub args: Vec<Argument>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub expression: Expr,
    pub filters: Vec<Filter>,
    /// The source of the expression and its filters, as written.
    pub raw_source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderVariable {
    /// `for` or `with`.
    pub kind: String,
    pub name: Expr,
}

/// What stands between the name of a tag and its closing delimiter.
#[derive(Debug, Clone, PartialEq)]
pub enum Markup {
    /// The markup was not understood, or the tag is not one the parser knows: kept as written.
    Raw(String),
    /// `ifchanged`: nothing.
    Nothing,
    Variable(Variable),
    Assign {
        name: String,
        value: Variable,
    },
    Cycle {
        group_name: Option<Expr>,
        args: Vec<Expr>,
    },
    For {
        variable_name: String,
        collection: Expr,
        reversed: bool,
        args: Vec<Argument>,
    },
    Paginate {
        collection: Expr,
        page_size: Expr,
        args: Vec<Argument>,
    },
    ContentFor {
        content_for_type: Expr,
        args: Vec<Argument>,
    },
    Block {
        name: Expr,
        args: Vec<Argument>,
    },
    Section {
        name: Expr,
        args: Vec<Argument>,
    },
    Render {
        snippet: Expr,
        variable: Option<RenderVariable>,
        alias: Option<String>,
        args: Vec<Argument>,
    },
    /// `capture`, `increment`, `decrement`, `layout`, `sections`, `partial`, `case`.
    Expression(Expr),
    /// `if`, `unless`, `elsif`.
    Condition(Expr),
    /// `form`.
    Arguments(Vec<Argument>),
    /// `when`.
    Expressions(Vec<Expr>),
    /// `liquid`: one tag per line.
    Statements(Vec<Id>),
}

impl Markup {
    pub fn is_raw(&self) -> bool {
        matches!(self, Markup::Raw(_))
    }

    pub fn raw(&self) -> Option<&str> {
        match self {
            Markup::Raw(markup) => Some(markup),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Kind {
    Document {
        children: Vec<Id>,
    },
    YamlFrontmatter {
        body: String,
    },
    LiquidRawTag {
        name: String,
        markup: String,
        body: Id,
        whitespace_start: bool,
        whitespace_end: bool,
        delimiter_whitespace_start: bool,
        delimiter_whitespace_end: bool,
        block_start: Span,
        block_end: Span,
    },
    LiquidTag {
        name: String,
        markup: Markup,
        children: Option<Vec<Id>>,
        whitespace_start: bool,
        whitespace_end: bool,
        /// The whitespace control of the end tag, when the tag has one.
        delimiter_whitespace: Option<(bool, bool)>,
        block_start: Span,
        block_end: Option<Span>,
    },
    LiquidBranch {
        /// `None` for what comes before the first `else`, `elsif` or `when`.
        name: Option<String>,
        markup: Markup,
        children: Vec<Id>,
        whitespace_start: bool,
        whitespace_end: bool,
        block_start: Span,
        block_end: Span,
    },
    LiquidVariableOutput {
        markup: Markup,
        whitespace_start: bool,
        whitespace_end: bool,
    },
    Html {
        kind: HtmlKind,
        /// The name of an element, a self-closing element or a stray closing tag, which may hold
        /// Liquid.
        name: Vec<Id>,
        /// The name of a void element or of a raw one (`script`, `style`, `svg`).
        plain_name: Option<String>,
        attributes: Vec<Id>,
        children: Vec<Id>,
        body: Option<Id>,
        block_start: Span,
        block_end: Option<Span>,
    },
    HtmlDoctype {
        legacy: Option<String>,
    },
    HtmlComment {
        body: String,
    },
    Attr {
        kind: AttrKind,
        name: Vec<Id>,
        value: Vec<Id>,
        /// The value, without its quotes.
        attribute_position: Span,
    },
    Text {
        value: String,
    },
    RawMarkup {
        kind: RawKind,
        value: String,
        nodes: Vec<Id>,
    },
    DocParam {
        name: String,
        description: Option<String>,
        param_type: Option<String>,
        required: bool,
    },
    DocExample {
        content: String,
        inline: bool,
    },
    DocDescription {
        content: String,
        implicit: bool,
        inline: bool,
    },
    DocPrompt {
        content: String,
    },
    /// A stretch of source the parser could not make sense of, printed as it is.
    Error,
}

#[derive(Debug, Clone, Default)]
pub struct Whitespace {
    pub is_dangling_sensitive: bool,
    pub is_whitespace_sensitive: bool,
    pub is_leading_sensitive: bool,
    pub is_trailing_sensitive: bool,
    pub is_indentation_sensitive: bool,
    pub has_leading: bool,
    pub has_trailing: bool,
    pub has_dangling: bool,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: Kind,
    pub position: Span,
    pub parent: Option<Id>,
    pub prev: Option<Id>,
    pub next: Option<Id>,
    pub first_child: Option<Id>,
    pub last_child: Option<Id>,
    pub css_display: String,
    pub css_whitespace: String,
    pub whitespace: Whitespace,
}

impl Node {
    pub fn is_html(&self) -> bool {
        matches!(self.kind, Kind::Html { .. })
    }

    pub fn html_kind(&self) -> Option<HtmlKind> {
        match self.kind {
            Kind::Html { kind, .. } => Some(kind),
            _ => None,
        }
    }

    pub fn is_html_kind(&self, expected: HtmlKind) -> bool {
        self.html_kind() == Some(expected)
    }

    pub fn is_liquid(&self) -> bool {
        matches!(
            self.kind,
            Kind::LiquidTag { .. }
                | Kind::LiquidVariableOutput { .. }
                | Kind::LiquidBranch { .. }
                | Kind::LiquidRawTag { .. }
        )
    }

    pub fn is_text(&self) -> bool {
        matches!(self.kind, Kind::Text { .. })
    }

    pub fn is_comment(&self) -> bool {
        matches!(self.kind, Kind::HtmlComment { .. })
    }

    pub fn is_liquid_tag(&self) -> bool {
        matches!(self.kind, Kind::LiquidTag { .. })
    }

    pub fn is_branch(&self) -> bool {
        matches!(self.kind, Kind::LiquidBranch { .. })
    }

    /// The children of the nodes that have some: the document, elements, tags and branches.
    pub fn children(&self) -> Option<&[Id]> {
        match &self.kind {
            Kind::Document { children } | Kind::LiquidBranch { children, .. } => Some(children),
            Kind::Html {
                kind: HtmlKind::Element,
                children,
                ..
            } => Some(children),
            Kind::LiquidTag { children, .. } => children.as_deref(),
            _ => None,
        }
    }

    pub fn attributes(&self) -> Option<&[Id]> {
        match &self.kind {
            Kind::Html {
                kind, attributes, ..
            } if *kind != HtmlKind::DanglingMarkerClose => Some(attributes),
            _ => None,
        }
    }

    /// The name of a Liquid tag or branch, or of a void or raw HTML element.
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            Kind::LiquidTag { name, .. } | Kind::LiquidRawTag { name, .. } => Some(name),
            Kind::LiquidBranch { name, .. } => name.as_deref(),
            Kind::Html { plain_name, .. } => plain_name.as_deref(),
            _ => None,
        }
    }

    pub fn tag_name_is(&self, expected: &str) -> bool {
        matches!(&self.kind, Kind::LiquidTag { name, .. } if name == expected)
    }

    pub fn block_start(&self) -> Option<Span> {
        match &self.kind {
            Kind::LiquidRawTag { block_start, .. }
            | Kind::LiquidTag { block_start, .. }
            | Kind::LiquidBranch { block_start, .. }
            | Kind::Html { block_start, .. } => Some(*block_start),
            _ => None,
        }
    }

    pub fn block_end(&self) -> Option<Span> {
        match &self.kind {
            Kind::LiquidRawTag { block_end, .. } | Kind::LiquidBranch { block_end, .. } => {
                Some(*block_end)
            }
            Kind::LiquidTag { block_end, .. } | Kind::Html { block_end, .. } => *block_end,
            _ => None,
        }
    }

    /// `{%-` or `{{-`.
    pub fn whitespace_start(&self) -> bool {
        match &self.kind {
            Kind::LiquidRawTag {
                whitespace_start, ..
            }
            | Kind::LiquidTag {
                whitespace_start, ..
            }
            | Kind::LiquidBranch {
                whitespace_start, ..
            }
            | Kind::LiquidVariableOutput {
                whitespace_start, ..
            } => *whitespace_start,
            _ => false,
        }
    }

    /// `-%}` or `-}}`.
    pub fn whitespace_end(&self) -> bool {
        match &self.kind {
            Kind::LiquidRawTag { whitespace_end, .. }
            | Kind::LiquidTag { whitespace_end, .. }
            | Kind::LiquidBranch { whitespace_end, .. }
            | Kind::LiquidVariableOutput { whitespace_end, .. } => *whitespace_end,
            _ => false,
        }
    }

    /// The whitespace control of the end tag: `{%- endif -%}`.
    pub fn delimiter_whitespace(&self) -> Option<(bool, bool)> {
        match &self.kind {
            Kind::LiquidRawTag {
                delimiter_whitespace_start,
                delimiter_whitespace_end,
                ..
            } => Some((*delimiter_whitespace_start, *delimiter_whitespace_end)),
            Kind::LiquidTag {
                delimiter_whitespace,
                ..
            } => *delimiter_whitespace,
            _ => None,
        }
    }

    pub fn markup(&self) -> Option<&Markup> {
        match &self.kind {
            Kind::LiquidTag { markup, .. }
            | Kind::LiquidBranch { markup, .. }
            | Kind::LiquidVariableOutput { markup, .. } => Some(markup),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match &self.kind {
            Kind::Text { value } => Some(value),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Ast {
    pub nodes: Vec<Node>,
    pub root: Id,
}

impl Ast {
    pub fn add(&mut self, kind: Kind, position: Span) -> Id {
        self.nodes.push(Node {
            kind,
            position,
            parent: None,
            prev: None,
            next: None,
            first_child: None,
            last_child: None,
            css_display: String::new(),
            css_whitespace: String::new(),
            whitespace: Whitespace::default(),
        });
        self.nodes.len() - 1
    }

    pub fn get(&self, id: Id) -> &Node {
        &self.nodes[id]
    }

    pub fn parent(&self, id: Id) -> Option<&Node> {
        self.nodes[id].parent.map(|parent| &self.nodes[parent])
    }

    pub fn prev(&self, id: Id) -> Option<&Node> {
        self.nodes[id].prev.map(|prev| &self.nodes[prev])
    }

    pub fn next(&self, id: Id) -> Option<&Node> {
        self.nodes[id].next.map(|next| &self.nodes[next])
    }

    /// The single text segment an element is named with, if that is all its name is.
    pub fn plain_compound_name(&self, id: Id) -> Option<&str> {
        match &self.nodes[id].kind {
            Kind::Html { name, .. } if name.len() == 1 => self.nodes[name[0]].text(),
            _ => None,
        }
    }

    /// Whether the node is one of the attributes of an HTML node.
    pub fn is_attribute_node(&self, id: Id) -> bool {
        self.parent(id)
            .and_then(|parent| parent.attributes())
            .is_some_and(|attributes| attributes.contains(&id))
    }

    pub fn is_branched_tag(&self, id: Id) -> bool {
        matches!(&self.nodes[id].kind, Kind::LiquidTag { name, .. }
            if matches!(name.as_str(), "if" | "for" | "unless" | "case"))
    }

    /// Every list of nodes a node holds, in the order Shopify's parser declares them.
    pub fn node_lists(&self, id: Id) -> Vec<&[Id]> {
        match &self.nodes[id].kind {
            Kind::Document { children } | Kind::LiquidBranch { children, .. } => vec![children],
            Kind::LiquidTag {
                markup, children, ..
            } => {
                let mut lists: Vec<&[Id]> = Vec::new();
                if let Markup::Statements(statements) = markup {
                    lists.push(statements);
                }
                if let Some(children) = children {
                    lists.push(children);
                }
                lists
            }
            Kind::Html {
                name,
                attributes,
                children,
                ..
            } => vec![name, attributes, children],
            Kind::Attr { name, value, .. } => vec![name, value],
            Kind::RawMarkup { nodes, .. } => vec![nodes],
            _ => Vec::new(),
        }
    }

    /// The node a node holds on its own: the body of a raw tag or of a raw element.
    pub fn single_child(&self, id: Id) -> Option<Id> {
        match &self.nodes[id].kind {
            Kind::LiquidRawTag { body, .. } => Some(*body),
            Kind::Html { body, .. } => *body,
            _ => None,
        }
    }
}

/// What JavaScript's `\s` matches, which is what the reference implementation trims and splits
/// on.
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

pub fn trim(text: &str) -> &str {
    text.trim_matches(is_space)
}

pub fn trim_start(text: &str) -> &str {
    text.trim_start_matches(is_space)
}

pub fn trim_end(text: &str) -> &str {
    text.trim_end_matches(is_space)
}

pub fn is_blank(text: &str) -> bool {
    text.chars().all(is_space)
}
