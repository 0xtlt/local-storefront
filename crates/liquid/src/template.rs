//! The parsed form of a template and how it renders.

use std::sync::Arc;

use crate::context::Context;
use crate::environment::Environment;
use crate::error::Result;
use crate::parser::Parser;
use crate::variable::Variable;

/// A tag instance in a parsed template.
pub trait Tag: Send + Sync {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()>;

    /// Blank tags produce no output of their own (`assign`, `comment`, ...). A block whose
    /// children are all blank drops its whitespace-only text, and errors raised by blank tags
    /// are recorded but not printed.
    fn blank(&self) -> bool {
        false
    }
}

pub enum Node {
    Text(Box<str>),
    Output { variable: Variable, line: u32 },
    Tag { tag: Box<dyn Tag>, line: u32 },
}

/// A sequence of nodes: the body of a template or of a block tag.
#[derive(Default)]
pub struct BlockBody {
    pub nodes: Vec<Node>,
    blank: bool,
}

impl BlockBody {
    pub fn new() -> Self {
        BlockBody {
            nodes: Vec::new(),
            blank: true,
        }
    }

    pub fn is_blank(&self) -> bool {
        self.blank
    }

    pub(crate) fn mark_not_blank(&mut self) {
        self.blank = false;
    }

    pub(crate) fn update_blank(&mut self, node_is_blank: bool) {
        self.blank = self.blank && node_is_blank;
    }

    /// Drops text nodes. Only meaningful on a blank body, where they are all whitespace.
    pub fn remove_blank_strings(&mut self) {
        self.nodes.retain(|node| !matches!(node, Node::Text(_)));
    }

    pub fn render(&self, ctx: &mut Context, out: &mut String) {
        let profiler = ctx.profiler().cloned();
        for node in &self.nodes {
            let line = match node {
                Node::Text(text) => {
                    out.push_str(text);
                    continue;
                }
                Node::Output { line, .. } | Node::Tag { line, .. } => *line,
            };
            // A profile counts every tag and every output, and when it is one of lines it
            // names each by its template and its line.
            let _span = profiler.as_ref().and_then(|profiler| {
                let span = profiler.lines().then(|| {
                    let template = ctx.template_name.as_deref().unwrap_or("(template)");
                    profiler.span(template, Some(line))
                });
                profiler.node();
                span
            });
            match node {
                Node::Text(_) => {}
                Node::Output { variable, line } => match variable.evaluate(ctx) {
                    Ok(value) => value.render_to(out),
                    Err(error) => out.push_str(&ctx.handle_error(error, *line)),
                },
                Node::Tag { tag, line } => {
                    if let Err(error) = tag.render(ctx, out) {
                        let message = ctx.handle_error(error, *line);
                        if !tag.blank() {
                            out.push_str(&message);
                        }
                    }
                }
            }
            if ctx.has_interrupt() {
                break;
            }
        }
    }

    pub fn render_to_string(&self, ctx: &mut Context) -> String {
        let mut out = String::new();
        self.render(ctx, &mut out);
        out
    }
}

pub struct Template {
    /// The name used in error messages, e.g. `snippets/price`.
    pub name: Option<Arc<str>>,
    pub root: BlockBody,
}

impl Template {
    pub fn parse(env: &Environment, source: &str) -> Result<Template> {
        Self::parse_named(env, source, None)
    }

    pub fn parse_named(env: &Environment, source: &str, name: Option<&str>) -> Result<Template> {
        let name: Option<Arc<str>> = name.map(Arc::from);
        let mut parser = Parser::new(env, source);
        match parser.parse_document() {
            Ok(root) => Ok(Template { name, root }),
            Err(error) => Err(error.with_line(parser.line_number()).with_template(name)),
        }
    }

    pub fn render(&self, ctx: &mut Context) -> String {
        let mut out = String::new();
        self.render_to(ctx, &mut out);
        out
    }

    /// Renders with this template's name as the current template, restoring the previous one
    /// afterwards.
    pub fn render_to(&self, ctx: &mut Context, out: &mut String) {
        let previous = std::mem::replace(&mut ctx.template_name, self.name.clone());
        // A template that has a name is a frame of the profile, when there is one.
        let span = match (ctx.profiler(), &self.name) {
            (Some(profiler), Some(name)) => Some(profiler.span(name, None)),
            _ => None,
        };
        self.root.render(ctx, out);
        drop(span);
        ctx.template_name = previous;
    }
}
