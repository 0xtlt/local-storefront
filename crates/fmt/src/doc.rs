//! The document a printer builds and the algorithm that lays it out: a port of Prettier's
//! `doc` builders and of `printDocToString`, kept faithful to it so that a document built the
//! way a Prettier plugin builds it breaks its lines at the same places.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

/// Names a group, so that an [`Doc::IfBreak`] elsewhere can ask how it was printed.
pub type GroupId = u32;

#[derive(Debug, Clone, PartialEq)]
pub enum Align {
    /// `dedentToRoot`: back to the indentation of the enclosing [`Align::Root`], or to none.
    ToRoot,
    /// `markAsRoot`: what `ToRoot` and literal lines come back to.
    Root,
    /// `align(n)`: `n` more columns.
    Width(usize),
    /// `dedent`: one level less.
    Dedent,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Doc {
    Text(String),
    Concat(Vec<Doc>),
    Indent(Box<Doc>),
    Align(Align, Box<Doc>),
    Group {
        contents: Box<Doc>,
        id: Option<GroupId>,
        should_break: bool,
        /// `conditionalGroup`: the states tried in turn, the last one being the broken one.
        expanded_states: Option<Vec<Doc>>,
    },
    Fill(Vec<Doc>),
    IfBreak {
        break_contents: Box<Doc>,
        flat_contents: Box<Doc>,
        group_id: Option<GroupId>,
    },
    IndentIfBreak {
        contents: Box<Doc>,
        group_id: GroupId,
        negate: bool,
    },
    Line {
        hard: bool,
        soft: bool,
        literal: bool,
    },
    LineSuffix(Box<Doc>),
    LineSuffixBoundary,
    BreakParent,
    Trim,
    /// A document that depends on the width of the indentation it is printed at: what an
    /// embedded formatter lays out on its own. It holds a hard line.
    AtIndent(Rc<AtIndentDoc>),
}

/// What [`Doc::AtIndent`] holds: how to build the document, and the document once built.
pub struct AtIndentDoc {
    build: Box<dyn Fn(usize) -> Doc>,
    built: OnceCell<Doc>,
}

impl AtIndentDoc {
    fn get(&self, indentation: usize) -> &Doc {
        self.built.get_or_init(|| (self.build)(indentation))
    }
}

impl fmt::Debug for AtIndentDoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AtIndentDoc")
            .field("built", &self.built.get())
            .finish()
    }
}

impl PartialEq for AtIndentDoc {
    fn eq(&self, other: &AtIndentDoc) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Doc {
    pub fn is_empty_text(&self) -> bool {
        matches!(self, Doc::Text(text) if text.is_empty())
    }

    /// Prettier's truthiness test of a doc: only the empty string is "nothing".
    pub fn is_nothing(&self) -> bool {
        self.is_empty_text()
    }
}

impl From<&str> for Doc {
    fn from(text: &str) -> Doc {
        Doc::Text(text.to_string())
    }
}

impl From<String> for Doc {
    fn from(text: String) -> Doc {
        Doc::Text(text)
    }
}

impl From<Vec<Doc>> for Doc {
    fn from(parts: Vec<Doc>) -> Doc {
        Doc::Concat(parts)
    }
}

pub fn text(value: impl Into<String>) -> Doc {
    Doc::Text(value.into())
}

pub fn nothing() -> Doc {
    Doc::Text(String::new())
}

pub fn concat(parts: Vec<Doc>) -> Doc {
    Doc::Concat(parts)
}

pub fn indent(contents: impl Into<Doc>) -> Doc {
    Doc::Indent(Box::new(contents.into()))
}

pub fn align(width: usize, contents: impl Into<Doc>) -> Doc {
    Doc::Align(Align::Width(width), Box::new(contents.into()))
}

pub fn dedent_to_root(contents: impl Into<Doc>) -> Doc {
    Doc::Align(Align::ToRoot, Box::new(contents.into()))
}

pub fn mark_as_root(contents: impl Into<Doc>) -> Doc {
    Doc::Align(Align::Root, Box::new(contents.into()))
}

pub fn dedent(contents: impl Into<Doc>) -> Doc {
    Doc::Align(Align::Dedent, Box::new(contents.into()))
}

pub fn group(contents: impl Into<Doc>) -> Doc {
    Doc::Group {
        contents: Box::new(contents.into()),
        id: None,
        should_break: false,
        expanded_states: None,
    }
}

pub fn group_with(contents: impl Into<Doc>, id: Option<GroupId>, should_break: bool) -> Doc {
    Doc::Group {
        contents: Box::new(contents.into()),
        id,
        should_break,
        expanded_states: None,
    }
}

pub fn conditional_group(states: Vec<Doc>) -> Doc {
    Doc::Group {
        contents: Box::new(states[0].clone()),
        id: None,
        should_break: false,
        expanded_states: Some(states),
    }
}

pub fn fill(parts: Vec<Doc>) -> Doc {
    Doc::Fill(parts)
}

pub fn if_break(
    break_contents: impl Into<Doc>,
    flat_contents: impl Into<Doc>,
    group_id: Option<GroupId>,
) -> Doc {
    Doc::IfBreak {
        break_contents: Box::new(break_contents.into()),
        flat_contents: Box::new(flat_contents.into()),
        group_id,
    }
}

pub fn indent_if_break(contents: impl Into<Doc>, group_id: GroupId, negate: bool) -> Doc {
    Doc::IndentIfBreak {
        contents: Box::new(contents.into()),
        group_id,
        negate,
    }
}

/// A document built from the width, in columns, of the indentation it is printed at.
pub fn at_indent(build: impl Fn(usize) -> Doc + 'static) -> Doc {
    Doc::AtIndent(Rc::new(AtIndentDoc {
        build: Box::new(build),
        built: OnceCell::new(),
    }))
}

pub fn line_suffix(contents: impl Into<Doc>) -> Doc {
    Doc::LineSuffix(Box::new(contents.into()))
}

pub const LINE: Doc = Doc::Line {
    hard: false,
    soft: false,
    literal: false,
};

pub const SOFTLINE: Doc = Doc::Line {
    hard: false,
    soft: true,
    literal: false,
};

const HARDLINE_WITHOUT_BREAK_PARENT: Doc = Doc::Line {
    hard: true,
    soft: false,
    literal: false,
};

const LITERALLINE_WITHOUT_BREAK_PARENT: Doc = Doc::Line {
    hard: true,
    soft: false,
    literal: true,
};

pub fn line() -> Doc {
    LINE
}

pub fn softline() -> Doc {
    SOFTLINE
}

pub fn hardline() -> Doc {
    Doc::Concat(vec![HARDLINE_WITHOUT_BREAK_PARENT, Doc::BreakParent])
}

pub fn literalline() -> Doc {
    Doc::Concat(vec![LITERALLINE_WITHOUT_BREAK_PARENT, Doc::BreakParent])
}

pub fn is_hardline(doc: &Doc) -> bool {
    matches!(doc, Doc::Concat(parts) if parts.len() == 2
        && parts[0] == HARDLINE_WITHOUT_BREAK_PARENT
        && parts[1] == Doc::BreakParent)
}

pub fn join(separator: &Doc, docs: Vec<Doc>) -> Doc {
    let mut parts = Vec::with_capacity(docs.len() * 2);
    for (index, doc) in docs.into_iter().enumerate() {
        if index > 0 {
            parts.push(separator.clone());
        }
        parts.push(doc);
    }
    Doc::Concat(parts)
}

/// `replaceEndOfLine`: every `\n` of every string becomes a literal line.
pub fn replace_end_of_line(text: &str) -> Doc {
    replace_end_of_line_with(text, &literalline())
}

pub fn replace_end_of_line_with(text: &str, replacement: &Doc) -> Doc {
    if !text.contains('\n') {
        return Doc::Text(text.to_string());
    }
    join(
        replacement,
        text.split('\n')
            .map(|part| Doc::Text(part.to_string()))
            .collect(),
    )
}

fn map_doc(doc: Doc, f: &impl Fn(Doc) -> Doc) -> Doc {
    let mapped = match doc {
        Doc::Concat(parts) => Doc::Concat(parts.into_iter().map(|part| map_doc(part, f)).collect()),
        Doc::Fill(parts) => Doc::Fill(parts.into_iter().map(|part| map_doc(part, f)).collect()),
        Doc::Indent(contents) => Doc::Indent(Box::new(map_doc(*contents, f))),
        Doc::Align(kind, contents) => Doc::Align(kind, Box::new(map_doc(*contents, f))),
        Doc::LineSuffix(contents) => Doc::LineSuffix(Box::new(map_doc(*contents, f))),
        Doc::Group {
            contents,
            id,
            should_break,
            expanded_states,
        } => Doc::Group {
            contents: Box::new(map_doc(*contents, f)),
            id,
            should_break,
            expanded_states: expanded_states
                .map(|states| states.into_iter().map(|state| map_doc(state, f)).collect()),
        },
        Doc::IfBreak {
            break_contents,
            flat_contents,
            group_id,
        } => Doc::IfBreak {
            break_contents: Box::new(map_doc(*break_contents, f)),
            flat_contents: Box::new(map_doc(*flat_contents, f)),
            group_id,
        },
        Doc::IndentIfBreak {
            contents,
            group_id,
            negate,
        } => Doc::IndentIfBreak {
            contents: Box::new(map_doc(*contents, f)),
            group_id,
            negate,
        },
        other => other,
    };
    f(mapped)
}

/// `removeLines`: what the document is when nothing in it may break.
pub fn remove_lines(doc: Doc) -> Doc {
    map_doc(doc, &|doc| match doc {
        Doc::Line {
            hard: false, soft, ..
        } => Doc::Text(if soft { "" } else { " " }.to_string()),
        Doc::IfBreak { flat_contents, .. } => *flat_contents,
        other => other,
    })
}

fn strip_trailing_hardline_from_parts(mut parts: Vec<Doc>) -> Vec<Doc> {
    while parts.len() >= 2
        && matches!(parts[parts.len() - 2], Doc::Line { .. })
        && parts[parts.len() - 1] == Doc::BreakParent
    {
        parts.truncate(parts.len() - 2);
    }
    if let Some(last) = parts.pop() {
        parts.push(strip_trailing_hardline_from_doc(last));
    }
    parts
}

fn strip_trailing_hardline_from_doc(doc: Doc) -> Doc {
    match doc {
        Doc::Indent(contents) => Doc::Indent(Box::new(strip_trailing_hardline_from_doc(*contents))),
        Doc::IndentIfBreak {
            contents,
            group_id,
            negate,
        } => Doc::IndentIfBreak {
            contents: Box::new(strip_trailing_hardline_from_doc(*contents)),
            group_id,
            negate,
        },
        Doc::Group {
            contents,
            id,
            should_break,
            expanded_states,
        } => Doc::Group {
            contents: Box::new(strip_trailing_hardline_from_doc(*contents)),
            id,
            should_break,
            expanded_states,
        },
        Doc::LineSuffix(contents) => {
            Doc::LineSuffix(Box::new(strip_trailing_hardline_from_doc(*contents)))
        }
        Doc::IfBreak {
            break_contents,
            flat_contents,
            group_id,
        } => Doc::IfBreak {
            break_contents: Box::new(strip_trailing_hardline_from_doc(*break_contents)),
            flat_contents: Box::new(strip_trailing_hardline_from_doc(*flat_contents)),
            group_id,
        },
        Doc::Fill(parts) => Doc::Fill(strip_trailing_hardline_from_parts(parts)),
        Doc::Concat(parts) => Doc::Concat(strip_trailing_hardline_from_parts(parts)),
        Doc::Text(text) => Doc::Text(text.trim_end_matches(['\n', '\r']).to_string()),
        other => other,
    }
}

/// `cleanDoc`: drops what prints nothing and flattens nested concatenations, which is what
/// lets [`strip_trailing_hardline`] see the hard line a document ends with.
pub fn clean_doc(doc: Doc) -> Doc {
    map_doc(doc, &|doc| match doc {
        Doc::Fill(parts) => {
            if parts.iter().all(Doc::is_empty_text) {
                nothing()
            } else if parts.len() == 1 {
                parts.into_iter().next().unwrap()
            } else {
                Doc::Fill(parts)
            }
        }
        Doc::Group {
            contents,
            id,
            should_break,
            expanded_states,
        } => {
            if contents.is_nothing() && id.is_none() && !should_break && expanded_states.is_none() {
                return nothing();
            }
            let same = matches!(&*contents, Doc::Group { id: inner_id, should_break: inner_break, expanded_states: inner_states, .. }
                if *inner_id == id && *inner_break == should_break && inner_states.is_none() && expanded_states.is_none());
            if same {
                return *contents;
            }
            Doc::Group {
                contents,
                id,
                should_break,
                expanded_states,
            }
        }
        Doc::Align(kind, contents) => {
            if contents.is_nothing() {
                nothing()
            } else {
                Doc::Align(kind, contents)
            }
        }
        Doc::Indent(contents) => {
            if contents.is_nothing() {
                nothing()
            } else {
                Doc::Indent(contents)
            }
        }
        Doc::LineSuffix(contents) => {
            if contents.is_nothing() {
                nothing()
            } else {
                Doc::LineSuffix(contents)
            }
        }
        Doc::IfBreak {
            break_contents,
            flat_contents,
            group_id,
        } => {
            if break_contents.is_nothing() && flat_contents.is_nothing() {
                nothing()
            } else {
                Doc::IfBreak {
                    break_contents,
                    flat_contents,
                    group_id,
                }
            }
        }
        Doc::Concat(parts) => {
            let mut cleaned: Vec<Doc> = Vec::with_capacity(parts.len());
            for part in parts {
                if part.is_nothing() {
                    continue;
                }
                let (first, rest) = match part {
                    Doc::Concat(inner) => {
                        let mut inner = inner.into_iter();
                        match inner.next() {
                            Some(first) => (first, inner.collect::<Vec<_>>()),
                            None => continue,
                        }
                    }
                    other => (other, Vec::new()),
                };
                match (cleaned.last_mut(), first) {
                    (Some(Doc::Text(previous)), Doc::Text(current)) => previous.push_str(&current),
                    (_, first) => cleaned.push(first),
                }
                cleaned.extend(rest);
            }
            match cleaned.len() {
                0 => nothing(),
                1 => cleaned.into_iter().next().unwrap(),
                _ => Doc::Concat(cleaned),
            }
        }
        other => other,
    })
}

pub fn strip_trailing_hardline(doc: Doc) -> Doc {
    strip_trailing_hardline_from_doc(clean_doc(doc))
}

/// `propagateBreaks`: a group that holds a hard line, or a group that breaks, breaks.
fn propagate_breaks(doc: &mut Doc) -> bool {
    match doc {
        Doc::BreakParent => true,
        Doc::Concat(parts) | Doc::Fill(parts) => {
            let mut breaks = false;
            for part in parts {
                breaks |= propagate_breaks(part);
            }
            breaks
        }
        Doc::Indent(contents) | Doc::Align(_, contents) | Doc::LineSuffix(contents) => {
            propagate_breaks(contents)
        }
        Doc::IndentIfBreak { contents, .. } => propagate_breaks(contents),
        Doc::IfBreak {
            break_contents,
            flat_contents,
            ..
        } => {
            let in_break = propagate_breaks(break_contents);
            let in_flat = propagate_breaks(flat_contents);
            in_break || in_flat
        }
        Doc::Group {
            contents,
            should_break,
            expanded_states,
            ..
        } => {
            let child_breaks = propagate_breaks(contents);
            if let Some(states) = expanded_states {
                for state in states {
                    propagate_breaks(state);
                }
                // A conditional group is never broken from the inside.
                return *should_break;
            }
            if child_breaks {
                *should_break = true;
            }
            *should_break
        }
        Doc::AtIndent(_) => true,
        Doc::Text(_) | Doc::Line { .. } | Doc::LineSuffixBoundary | Doc::Trim => false,
    }
}

/// The width a string takes on a line: wide characters count for two columns, combining ones
/// for none.
pub fn string_width(text: &str) -> usize {
    if text.bytes().all(|byte| (0x20..0x7f).contains(&byte)) {
        return text.len();
    }
    text.chars().map(char_width).sum()
}

fn char_width(c: char) -> usize {
    let code = c as u32;
    if code <= 31 || (127..=159).contains(&code) {
        return 0;
    }
    if (768..=879).contains(&code) || (65024..=65039).contains(&code) || code == 0x200d {
        return 0;
    }
    let wide = matches!(code,
        0x1100..=0x115f
        | 0x231a..=0x231b
        | 0x2329..=0x232a
        | 0x23e9..=0x23ec
        | 0x23f0
        | 0x23f3
        | 0x25fd..=0x25fe
        | 0x2614..=0x2615
        | 0x2648..=0x2653
        | 0x267f
        | 0x2693
        | 0x26a1
        | 0x26aa..=0x26ab
        | 0x26bd..=0x26be
        | 0x26c4..=0x26c5
        | 0x26ce
        | 0x26d4
        | 0x26ea
        | 0x26f2..=0x26f3
        | 0x26f5
        | 0x26fa
        | 0x26fd
        | 0x2705
        | 0x270a..=0x270b
        | 0x2728
        | 0x274c
        | 0x274e
        | 0x2753..=0x2755
        | 0x2757
        | 0x2795..=0x2797
        | 0x27b0
        | 0x27bf
        | 0x2b1b..=0x2b1c
        | 0x2b50
        | 0x2b55
        | 0x2e80..=0x303e
        | 0x3041..=0x33ff
        | 0x3400..=0x4dbf
        | 0x4e00..=0x9fff
        | 0xa000..=0xa4cf
        | 0xa960..=0xa97f
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe10..=0xfe19
        | 0xfe30..=0xfe6f
        | 0xff00..=0xff60
        | 0xffe0..=0xffe6
        | 0x1f004
        | 0x1f0cf
        | 0x1f18e
        | 0x1f191..=0x1f19a
        | 0x1f200..=0x1f64f
        | 0x1f680..=0x1f6ff
        | 0x1f900..=0x1f9ff
        | 0x1fa70..=0x1faff
        | 0x20000..=0x3fffd);
    if wide { 2 } else { 1 }
}

pub struct PrintOptions {
    pub print_width: usize,
    pub tab_width: usize,
    pub use_tabs: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Break,
    Flat,
}

#[derive(Clone, Debug)]
enum IndentCommand {
    Indent,
    Width(usize),
}

#[derive(Debug)]
struct Indent {
    value: String,
    length: usize,
    queue: Vec<IndentCommand>,
    root: Option<Rc<Indent>>,
}

fn generate_indent(
    indent: &Rc<Indent>,
    command: Option<IndentCommand>,
    options: &PrintOptions,
) -> Rc<Indent> {
    let mut queue = indent.queue.clone();
    match command {
        Some(command) => queue.push(command),
        None => {
            queue.pop();
        }
    }
    let mut value = String::new();
    let mut length = 0;
    let mut last_tabs = 0;
    let mut last_spaces = 0;
    for command in &queue {
        match command {
            IndentCommand::Indent => {
                // flush
                if options.use_tabs {
                    if last_tabs > 0 {
                        value.push_str(&"\t".repeat(last_tabs));
                        length += options.tab_width * last_tabs;
                    }
                } else if last_spaces > 0 {
                    value.push_str(&" ".repeat(last_spaces));
                    length += last_spaces;
                }
                last_tabs = 0;
                last_spaces = 0;
                if options.use_tabs {
                    value.push('\t');
                    length += options.tab_width;
                } else {
                    value.push_str(&" ".repeat(options.tab_width));
                    length += options.tab_width;
                }
            }
            IndentCommand::Width(width) => {
                last_tabs += 1;
                last_spaces += width;
            }
        }
    }
    if last_spaces > 0 {
        value.push_str(&" ".repeat(last_spaces));
        length += last_spaces;
    }
    Rc::new(Indent {
        value,
        length,
        queue,
        root: indent.root.clone(),
    })
}

fn make_align(indent: &Rc<Indent>, kind: &Align, options: &PrintOptions) -> Rc<Indent> {
    match kind {
        Align::Root => Rc::new(Indent {
            value: indent.value.clone(),
            length: indent.length,
            queue: indent.queue.clone(),
            root: Some(indent.clone()),
        }),
        Align::ToRoot => indent.root.clone().unwrap_or_else(root_indent),
        Align::Dedent => generate_indent(indent, None, options),
        Align::Width(width) => generate_indent(indent, Some(IndentCommand::Width(*width)), options),
    }
}

fn root_indent() -> Rc<Indent> {
    Rc::new(Indent {
        value: String::new(),
        length: 0,
        queue: Vec::new(),
        root: None,
    })
}

#[derive(Clone)]
enum Item<'a> {
    Doc(&'a Doc),
    /// What is left of a fill once its first parts are printed.
    FillRest(&'a [Doc]),
    /// A document made up while printing: an indented `indentIfBreak`.
    Indented(&'a Doc),
}

#[derive(Clone)]
struct Command<'a> {
    indent: Rc<Indent>,
    mode: Mode,
    item: Item<'a>,
}

fn trailing_indentation(text: &str) -> usize {
    text.bytes()
        .rev()
        .take_while(|byte| *byte == b' ' || *byte == b'\t')
        .count()
}

#[allow(clippy::too_many_arguments)]
fn fits<'a>(
    next: &[(Mode, Item<'a>)],
    rest_commands: &[Command<'a>],
    mut remaining_width: isize,
    mut has_line_suffix: bool,
    group_modes: &HashMap<GroupId, Mode>,
    must_be_flat: bool,
) -> bool {
    let mut rest_index = rest_commands.len();
    let mut has_pending_space = false;
    let mut commands: Vec<(Mode, Item<'a>)> = next.iter().rev().cloned().collect();
    let mut output = String::new();
    while remaining_width >= 0 {
        let Some((mode, item)) = commands.pop() else {
            if rest_index == 0 {
                return true;
            }
            rest_index -= 1;
            let command = &rest_commands[rest_index];
            commands.push((command.mode, command.item.clone()));
            continue;
        };
        let doc = match item {
            Item::Doc(doc) => doc,
            Item::Indented(doc) => doc,
            Item::FillRest(parts) => {
                for part in parts.iter().rev() {
                    commands.push((mode, Item::Doc(part)));
                }
                continue;
            }
        };
        match doc {
            Doc::Text(text) => {
                if !text.is_empty() {
                    if has_pending_space {
                        output.push(' ');
                        remaining_width -= 1;
                        has_pending_space = false;
                    }
                    output.push_str(text);
                    remaining_width -= string_width(text) as isize;
                }
            }
            Doc::Concat(parts) | Doc::Fill(parts) => {
                for part in parts.iter().rev() {
                    commands.push((mode, Item::Doc(part)));
                }
            }
            Doc::Indent(contents) | Doc::Align(_, contents) => {
                commands.push((mode, Item::Doc(contents)));
            }
            Doc::IndentIfBreak { contents, .. } => commands.push((mode, Item::Doc(contents))),
            Doc::Trim => {
                let count = trailing_indentation(&output);
                output.truncate(output.len() - count);
                remaining_width += count as isize;
            }
            Doc::Group {
                contents,
                should_break,
                expanded_states,
                ..
            } => {
                if must_be_flat && *should_break {
                    return false;
                }
                let group_mode = if *should_break { Mode::Break } else { mode };
                let contents = match expanded_states {
                    Some(states) if group_mode == Mode::Break => states.last().unwrap(),
                    _ => contents,
                };
                commands.push((group_mode, Item::Doc(contents)));
            }
            Doc::IfBreak {
                break_contents,
                flat_contents,
                group_id,
            } => {
                let group_mode = match group_id {
                    Some(id) => group_modes.get(id).copied().unwrap_or(Mode::Flat),
                    None => mode,
                };
                let contents = if group_mode == Mode::Break {
                    break_contents
                } else {
                    flat_contents
                };
                if !contents.is_nothing() {
                    commands.push((mode, Item::Doc(contents)));
                }
            }
            Doc::Line { hard, soft, .. } => {
                if mode == Mode::Break || *hard {
                    return true;
                }
                if !soft {
                    has_pending_space = true;
                }
            }
            Doc::LineSuffix(_) => has_line_suffix = true,
            Doc::LineSuffixBoundary => {
                if has_line_suffix {
                    return false;
                }
            }
            Doc::BreakParent => {}
            // It holds a hard line, which ends the line being measured.
            Doc::AtIndent(_) => return !must_be_flat,
        }
    }
    false
}

/// Lays a document out in `print_width` columns.
pub fn print(mut doc: Doc, options: &PrintOptions) -> String {
    propagate_breaks(&mut doc);
    let doc = &doc;
    let width = options.print_width as isize;
    let mut group_modes: HashMap<GroupId, Mode> = HashMap::new();
    let mut position: isize = 0;
    let mut should_remeasure = false;
    let mut out = String::new();
    let mut line_suffixes: Vec<Command> = Vec::new();
    let mut commands: Vec<Command> = vec![Command {
        indent: root_indent(),
        mode: Mode::Break,
        item: Item::Doc(doc),
    }];

    while let Some(Command { indent, mode, item }) = commands.pop() {
        let doc = match item {
            Item::Doc(doc) => doc,
            Item::Indented(doc) => {
                commands.push(Command {
                    indent: generate_indent(&indent, Some(IndentCommand::Indent), options),
                    mode,
                    item: Item::Doc(doc),
                });
                continue;
            }
            Item::FillRest(parts) => {
                print_fill(
                    parts,
                    &indent,
                    mode,
                    &mut commands,
                    width - position,
                    !line_suffixes.is_empty(),
                    &group_modes,
                );
                continue;
            }
        };
        match doc {
            Doc::Text(text) => {
                if !text.is_empty() {
                    out.push_str(text);
                    if !commands.is_empty() {
                        position += string_width(text) as isize;
                    }
                }
            }
            Doc::Concat(parts) => {
                for part in parts.iter().rev() {
                    commands.push(Command {
                        indent: indent.clone(),
                        mode,
                        item: Item::Doc(part),
                    });
                }
            }
            Doc::Indent(contents) => commands.push(Command {
                indent: generate_indent(&indent, Some(IndentCommand::Indent), options),
                mode,
                item: Item::Doc(contents),
            }),
            Doc::Align(kind, contents) => commands.push(Command {
                indent: make_align(&indent, kind, options),
                mode,
                item: Item::Doc(contents),
            }),
            Doc::Trim => {
                let count = trailing_indentation(&out);
                out.truncate(out.len() - count);
                position -= count as isize;
            }
            Doc::Group {
                contents,
                id,
                should_break,
                expanded_states,
            } => {
                let command = if mode == Mode::Flat && !should_remeasure {
                    Command {
                        indent: indent.clone(),
                        mode: if *should_break {
                            Mode::Break
                        } else {
                            Mode::Flat
                        },
                        item: Item::Doc(contents),
                    }
                } else {
                    should_remeasure = false;
                    let remaining = width - position;
                    let has_line_suffix = !line_suffixes.is_empty();
                    let flat = (Mode::Flat, Item::Doc(contents));
                    if !*should_break
                        && fits(
                            &[flat],
                            &commands,
                            remaining,
                            has_line_suffix,
                            &group_modes,
                            false,
                        )
                    {
                        Command {
                            indent: indent.clone(),
                            mode: Mode::Flat,
                            item: Item::Doc(contents),
                        }
                    } else if let Some(states) = expanded_states {
                        let mut chosen = None;
                        if !*should_break {
                            for state in &states[1..states.len() - 1] {
                                if fits(
                                    &[(Mode::Flat, Item::Doc(state))],
                                    &commands,
                                    remaining,
                                    has_line_suffix,
                                    &group_modes,
                                    false,
                                ) {
                                    chosen = Some(Command {
                                        indent: indent.clone(),
                                        mode: Mode::Flat,
                                        item: Item::Doc(state),
                                    });
                                    break;
                                }
                            }
                        }
                        chosen.unwrap_or_else(|| Command {
                            indent: indent.clone(),
                            mode: Mode::Break,
                            item: Item::Doc(states.last().unwrap()),
                        })
                    } else {
                        Command {
                            indent: indent.clone(),
                            mode: Mode::Break,
                            item: Item::Doc(contents),
                        }
                    }
                };
                if let Some(id) = id {
                    group_modes.insert(*id, command.mode);
                }
                commands.push(command);
            }
            Doc::Fill(parts) => print_fill(
                parts,
                &indent,
                mode,
                &mut commands,
                width - position,
                !line_suffixes.is_empty(),
                &group_modes,
            ),
            Doc::IfBreak {
                break_contents,
                flat_contents,
                group_id,
            } => {
                let group_mode = match group_id {
                    Some(id) => group_modes.get(id).copied(),
                    None => Some(mode),
                };
                let contents = match group_mode {
                    Some(Mode::Break) => Some(break_contents),
                    Some(Mode::Flat) => Some(flat_contents),
                    None => None,
                };
                if let Some(contents) = contents
                    && !contents.is_nothing()
                {
                    commands.push(Command {
                        indent,
                        mode,
                        item: Item::Doc(contents),
                    });
                }
            }
            Doc::IndentIfBreak {
                contents,
                group_id,
                negate,
            } => {
                let indented = match group_modes.get(group_id) {
                    Some(Mode::Break) => Some(!*negate),
                    Some(Mode::Flat) => Some(*negate),
                    None => None,
                };
                if let Some(indented) = indented {
                    commands.push(Command {
                        indent,
                        mode,
                        item: if indented {
                            Item::Indented(contents)
                        } else {
                            Item::Doc(contents)
                        },
                    });
                }
            }
            Doc::LineSuffix(contents) => line_suffixes.push(Command {
                indent,
                mode,
                item: Item::Doc(contents),
            }),
            Doc::LineSuffixBoundary => {
                if !line_suffixes.is_empty() {
                    commands.push(Command {
                        indent,
                        mode,
                        item: Item::Doc(&HARDLINE_WITHOUT_BREAK_PARENT),
                    });
                }
            }
            Doc::Line {
                hard,
                soft,
                literal,
            } => {
                if mode == Mode::Flat && !*hard {
                    if !*soft {
                        out.push(' ');
                        position += 1;
                    }
                } else {
                    if mode == Mode::Flat {
                        should_remeasure = true;
                    }
                    if !line_suffixes.is_empty() {
                        commands.push(Command {
                            indent,
                            mode,
                            item: Item::Doc(doc),
                        });
                        commands.extend(line_suffixes.drain(..).rev());
                    } else if *literal {
                        out.push('\n');
                        position = 0;
                        if let Some(root) = &indent.root {
                            out.push_str(&root.value);
                            position = root.length as isize;
                        }
                    } else {
                        let count = trailing_indentation(&out);
                        out.truncate(out.len() - count);
                        out.push('\n');
                        out.push_str(&indent.value);
                        position = indent.length as isize;
                    }
                }
            }
            Doc::BreakParent => {}
            Doc::AtIndent(built) => commands.push(Command {
                item: Item::Doc(built.get(indent.length)),
                indent,
                mode,
            }),
        }
        if commands.is_empty() && !line_suffixes.is_empty() {
            commands.extend(line_suffixes.drain(..).rev());
        }
    }
    out
}

fn print_fill<'a>(
    parts: &'a [Doc],
    indent: &Rc<Indent>,
    mode: Mode,
    commands: &mut Vec<Command<'a>>,
    remaining: isize,
    has_line_suffix: bool,
    group_modes: &HashMap<GroupId, Mode>,
) {
    if parts.is_empty() {
        return;
    }
    let command = |mode: Mode, doc: &'a Doc| Command {
        indent: indent.clone(),
        mode,
        item: Item::Doc(doc),
    };
    let content = &parts[0];
    let content_fits = fits(
        &[(Mode::Flat, Item::Doc(content))],
        &[],
        remaining,
        has_line_suffix,
        group_modes,
        true,
    );
    if parts.len() == 1 {
        commands.push(command(
            if content_fits {
                Mode::Flat
            } else {
                Mode::Break
            },
            content,
        ));
        return;
    }
    let whitespace = &parts[1];
    if parts.len() == 2 {
        let mode = if content_fits {
            Mode::Flat
        } else {
            Mode::Break
        };
        commands.push(command(mode, whitespace));
        commands.push(command(mode, content));
        return;
    }
    let second = &parts[2];
    commands.push(Command {
        indent: indent.clone(),
        mode,
        item: Item::FillRest(&parts[2..]),
    });
    let both_fit = fits(
        &[
            (Mode::Flat, Item::Doc(content)),
            (Mode::Flat, Item::Doc(whitespace)),
            (Mode::Flat, Item::Doc(second)),
        ],
        &[],
        remaining,
        has_line_suffix,
        group_modes,
        true,
    );
    if both_fit {
        commands.push(command(Mode::Flat, whitespace));
        commands.push(command(Mode::Flat, content));
    } else if content_fits {
        commands.push(command(Mode::Break, whitespace));
        commands.push(command(Mode::Flat, content));
    } else {
        commands.push(command(Mode::Break, whitespace));
        commands.push(command(Mode::Break, content));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(print_width: usize) -> PrintOptions {
        PrintOptions {
            print_width,
            tab_width: 2,
            use_tabs: false,
        }
    }

    #[test]
    fn a_group_breaks_only_when_it_does_not_fit() {
        let doc = || {
            group(vec![
                text("["),
                indent(vec![softline(), text("a,"), line(), text("b")]),
                softline(),
                text("]"),
            ])
        };
        assert_eq!(print(doc(), &options(80)), "[a, b]");
        assert_eq!(print(doc(), &options(4)), "[\n  a,\n  b\n]");
    }

    #[test]
    fn a_hard_line_breaks_the_groups_around_it() {
        let doc = group(vec![
            text("a"),
            line(),
            group(vec![text("b"), hardline(), text("c")]),
        ]);
        assert_eq!(print(doc, &options(80)), "a\nb\nc");
    }

    #[test]
    fn a_fill_puts_as_much_as_fits_on_each_line() {
        let words = "one two three four five six";
        let mut parts = Vec::new();
        for (index, word) in words.split(' ').enumerate() {
            if index > 0 {
                parts.push(line());
            }
            parts.push(text(word));
        }
        assert_eq!(
            print(fill(parts), &options(10)),
            "one two\nthree four\nfive six"
        );
    }

    #[test]
    fn if_break_follows_the_group_it_names() {
        let doc = vec![
            group_with(vec![text("a"), hardline(), text("b")], Some(1), false),
            if_break("!", "?", Some(1)),
            group_with(text("c"), Some(2), false),
            if_break("!", "?", Some(2)),
        ];
        assert_eq!(print(concat(doc), &options(80)), "a\nb!c?");
    }

    #[test]
    fn dedent_to_root_and_literal_lines_ignore_the_indentation() {
        let doc = indent(vec![
            hardline(),
            text("a"),
            dedent_to_root(vec![hardline(), text("b")]),
            literalline(),
            text("c"),
        ]);
        assert_eq!(print(doc, &options(80)), "\n  a\nb\nc");
    }

    #[test]
    fn strips_the_hard_line_a_document_ends_with() {
        let doc = concat(vec![text("a"), concat(vec![hardline()]), nothing()]);
        assert_eq!(print(strip_trailing_hardline(doc), &options(80)), "a");
    }
}
