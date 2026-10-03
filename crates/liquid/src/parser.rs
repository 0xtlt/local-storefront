//! The block-level parser: turns the token stream into nodes and dispatches tags.
//!
//! This follows Ruby Liquid's `BlockBody` closely, including whitespace control, the blank-body
//! rule and the way an unknown tag ends the current body and is handed to the enclosing block.

use crate::environment::Environment;
use crate::error::{Error, Result};
use crate::lax;
use crate::template::{BlockBody, Node};
use crate::tokenizer::Tokenizer;
use crate::variable::Variable;

/// The name and markup of a tag being parsed.
pub struct TagToken<'a> {
    pub name: &'a str,
    pub markup: &'a str,
}

/// A tag the current body does not know: either the delimiter of the enclosing block
/// (`endif`, `else`, ...) or a genuinely unknown tag.
#[derive(Debug)]
pub struct EndTag {
    pub name: String,
    pub markup: String,
}

pub struct Parser<'e, 's> {
    env: &'e Environment,
    tokenizer: Tokenizer<'s>,
    line_number: u32,
    trim_whitespace: bool,
    depth: usize,
}

const MAX_PARSE_DEPTH: usize = 100;

/// The pieces of `{% name markup %}`.
struct FullToken<'s> {
    name: &'s str,
    markup: &'s str,
    newlines_before_markup: u32,
}

/// `FullToken = /\A\{%-?(\s*)(#|\w+)(\s*)(.*?)-?%\}\z/m`.
fn full_token(token: &str) -> Option<FullToken<'_>> {
    let inner = token.strip_prefix("{%")?.strip_suffix("%}")?;
    let inner = inner.strip_prefix('-').unwrap_or(inner);
    let name_start = lax::skip_space(inner, 0);
    let rest = &inner[name_start..];
    let name_len = if rest.starts_with('#') {
        1
    } else {
        rest.char_indices()
            .find(|(_, c)| !lax::is_word(*c))
            .map_or(rest.len(), |(i, _)| i)
    };
    if name_len == 0 {
        return None;
    }
    let name = &rest[..name_len];
    let markup_start = lax::skip_space(rest, name_len);
    let markup = &rest[markup_start..];
    let markup = markup.strip_suffix('-').unwrap_or(markup);
    let newlines = |s: &str| s.bytes().filter(|b| *b == b'\n').count() as u32;
    Some(FullToken {
        name,
        markup,
        newlines_before_markup: newlines(&inner[..name_start])
            + newlines(&rest[name_len..markup_start]),
    })
}

/// `FullTokenPossiblyInvalid = /\A(.*)\{%-?\s*(\w+)\s*(.*)?-?%\}\z/m`: returns the text before
/// the tag and the tag name.
fn full_token_possibly_invalid(token: &str) -> Option<(&str, &str)> {
    if !token.ends_with("%}") {
        return None;
    }
    let mut search_end = token.len();
    while let Some(start) = token[..search_end].rfind("{%") {
        let inner = &token[start + 2..token.len() - 2];
        let inner = inner.strip_prefix('-').unwrap_or(inner);
        let rest = &inner[lax::skip_space(inner, 0)..];
        let name_len = rest
            .char_indices()
            .find(|(_, c)| !lax::is_word(*c))
            .map_or(rest.len(), |(i, _)| i);
        if name_len > 0 {
            return Some((&token[..start], &rest[..name_len]));
        }
        search_end = start;
    }
    None
}

/// `LiquidTagToken = /\A\s*(#|\w+)\s*(.*?)\z/`: a line inside `{% liquid %}`.
fn liquid_tag_token(line: &str) -> Option<(&str, &str)> {
    let rest = &line[lax::skip_space(line, 0)..];
    let name_len = if rest.starts_with('#') {
        1
    } else {
        rest.char_indices()
            .find(|(_, c)| !lax::is_word(*c))
            .map_or(rest.len(), |(i, _)| i)
    };
    if name_len == 0 {
        return None;
    }
    Some((&rest[..name_len], &rest[lax::skip_space(rest, name_len)..]))
}

fn trims_right(token: &str) -> bool {
    token.len() >= 3 && token.as_bytes()[token.len() - 3] == b'-'
}

impl<'e, 's> Parser<'e, 's> {
    pub fn new(env: &'e Environment, source: &'s str) -> Self {
        Parser {
            env,
            tokenizer: Tokenizer::new(source),
            line_number: 1,
            trim_whitespace: false,
            depth: 0,
        }
    }

    pub fn environment(&self) -> &'e Environment {
        self.env
    }

    /// The line of the token being parsed.
    pub fn line_number(&self) -> u32 {
        self.line_number
    }

    pub(crate) fn parse_document(&mut self) -> Result<BlockBody> {
        let mut body = BlockBody::new();
        match self.parse_body(&mut body)? {
            None => Ok(body),
            Some(end) => Err(match end.name.as_str() {
                "else" | "end" => Error::syntax(format!("Unexpected outer '{}' tag", end.name)),
                _ => Error::syntax(format!("Unknown tag '{}'", end.name)),
            }),
        }
    }

    /// Parses nodes into `body` until the input ends (`None`) or a tag that is not registered
    /// is met (`Some`), which the caller interprets as its own delimiter or as an error.
    pub fn parse_body(&mut self, body: &mut BlockBody) -> Result<Option<EndTag>> {
        if self.depth >= MAX_PARSE_DEPTH {
            return Err(Error::stack_level());
        }
        self.depth += 1;
        self.line_number = self.tokenizer.line_number();
        let result = if self.tokenizer.is_for_liquid_tag() {
            self.parse_for_liquid_tag(body)
        } else {
            self.parse_for_document(body)
        };
        self.depth -= 1;
        result
    }

    fn parse_for_document(&mut self, body: &mut BlockBody) -> Result<Option<EndTag>> {
        while let Some(token) = self.tokenizer.shift() {
            if token.is_empty() {
                continue;
            }
            if token.starts_with("{%") {
                self.handle_whitespace(token, body);
                let Some(full) = full_token(token) else {
                    if token.ends_with("%}") {
                        return Ok(Some(EndTag {
                            name: token.to_string(),
                            markup: token.to_string(),
                        }));
                    }
                    return Err(Error::syntax(format!(
                        "Tag '{token}' was not properly terminated with regexp: /\\%\\}}/"
                    )));
                };
                self.line_number += full.newlines_before_markup;
                if full.name == "liquid" {
                    self.parse_liquid_tag(full.markup, body)?;
                    continue;
                }
                let Some(tag_parser) = self.env.tag(full.name) else {
                    return Ok(Some(EndTag {
                        name: full.name.to_string(),
                        markup: full.markup.to_string(),
                    }));
                };
                let line = self.line_number;
                let tag = tag_parser(
                    self,
                    &TagToken {
                        name: full.name,
                        markup: full.markup,
                    },
                )?;
                body.update_blank(tag.blank());
                body.nodes.push(Node::Tag { tag, line });
            } else if token.starts_with("{{") {
                self.handle_whitespace(token, body);
                let variable = self.create_variable(token)?;
                body.nodes.push(Node::Output {
                    variable,
                    line: self.line_number,
                });
                body.mark_not_blank();
            } else {
                let text = if self.trim_whitespace {
                    lax::lstrip(token)
                } else {
                    token
                };
                self.trim_whitespace = false;
                body.update_blank(lax::is_blank(text));
                body.nodes.push(Node::Text(text.into()));
            }
            self.line_number = self.tokenizer.line_number();
        }
        Ok(None)
    }

    fn parse_for_liquid_tag(&mut self, body: &mut BlockBody) -> Result<Option<EndTag>> {
        while let Some(token) = self.tokenizer.shift() {
            if !lax::is_blank(token) {
                let Some((name, markup)) = liquid_tag_token(token) else {
                    return Ok(Some(EndTag {
                        name: token.to_string(),
                        markup: token.to_string(),
                    }));
                };
                if name == "liquid" {
                    self.line_number = self.line_number.saturating_sub(1);
                    self.parse_liquid_tag(markup, body)?;
                    continue;
                }
                let Some(tag_parser) = self.env.tag(name) else {
                    return Ok(Some(EndTag {
                        name: name.to_string(),
                        markup: markup.to_string(),
                    }));
                };
                let line = self.line_number;
                let tag = tag_parser(self, &TagToken { name, markup })?;
                body.update_blank(tag.blank());
                body.nodes.push(Node::Tag { tag, line });
            }
            self.line_number = self.tokenizer.line_number();
        }
        Ok(None)
    }

    /// Parses the body of a `{% liquid %}` tag into the current body.
    fn parse_liquid_tag(&mut self, markup: &'s str, body: &mut BlockBody) -> Result<()> {
        let liquid_tokenizer = Tokenizer::for_liquid_tag(markup, self.line_number);
        let outer = std::mem::replace(&mut self.tokenizer, liquid_tokenizer);
        let result = self.parse_for_liquid_tag(body);
        self.tokenizer = outer;
        match result? {
            None => Ok(()),
            Some(end) => Err(Self::unknown_tag_error(&end.name, "liquid", "%}")),
        }
    }

    fn handle_whitespace(&mut self, token: &str, body: &mut BlockBody) {
        if token.as_bytes().get(2) == Some(&b'-')
            && let Some(Node::Text(previous)) = body.nodes.last_mut()
        {
            let trimmed = lax::rstrip(previous);
            if trimmed.is_empty() && self.env.bug_compatible_whitespace_trimming() {
                // Text that was nothing but whitespace keeps its first character.
                let kept = previous.chars().next().map_or(0, char::len_utf8);
                if kept != previous.len() {
                    *previous = previous[..kept].into();
                }
            } else if trimmed.len() != previous.len() {
                *previous = trimmed.into();
            }
        }
        self.trim_whitespace = trims_right(token);
    }

    fn create_variable(&self, token: &str) -> Result<Variable> {
        let Some(inner) = token.strip_prefix("{{").and_then(|t| t.strip_suffix("}}")) else {
            return Err(Error::syntax(format!(
                "Variable '{token}' was not properly terminated with regexp: /\\}}\\}}/"
            )));
        };
        let inner = inner.strip_prefix('-').unwrap_or(inner);
        let inner = inner.strip_suffix('-').unwrap_or(inner);
        Ok(Variable::parse(inner))
    }

    // --- helpers for block tags ------------------------------------------------------------

    /// Parses a block with no inner delimiters: everything up to `end<name>`.
    pub fn parse_block(&mut self, block_name: &str) -> Result<BlockBody> {
        let mut body = BlockBody::new();
        let delimiter = format!("end{block_name}");
        match self.parse_body(&mut body)? {
            Some(end) if end.name == delimiter => Ok(body),
            Some(end) => Err(Self::unknown_tag_error(&end.name, block_name, &delimiter)),
            None => Err(Self::tag_never_closed(block_name)),
        }
    }

    /// Consumes raw tokens up to `end<name>` without parsing them, as `raw` does.
    pub fn parse_raw_body(&mut self, block_name: &str) -> Result<String> {
        self.raw_body(block_name, true)
    }

    /// Like [`Parser::parse_raw_body`], for a block that must not contain a tag of its own
    /// name (`doc`).
    pub fn parse_unnested_raw_body(&mut self, block_name: &str) -> Result<String> {
        self.raw_body(block_name, false)
    }

    fn raw_body(&mut self, block_name: &str, nestable: bool) -> Result<String> {
        let delimiter = format!("end{block_name}");
        let mut body = String::new();
        while let Some(token) = self.tokenizer.shift() {
            if let Some((before, name)) = full_token_possibly_invalid(token) {
                if !nestable && name == block_name {
                    return Err(Error::syntax(format!(
                        "Syntax Error in '{block_name}' - Nested {block_name} tags are not allowed"
                    )));
                }
                if name == delimiter {
                    self.trim_whitespace = trims_right(token);
                    body.push_str(before);
                    self.line_number = self.tokenizer.line_number();
                    return Ok(body);
                }
            }
            body.push_str(token);
        }
        Err(Self::tag_never_closed(block_name))
    }

    /// Skips the body of a `comment` block, honouring nested `comment` and `raw` tags.
    pub fn skip_comment_body(&mut self) -> Result<()> {
        let mut depth = 1usize;
        while let Some(token) = self.tokenizer.shift() {
            let name = if self.tokenizer.is_for_liquid_tag() {
                match liquid_tag_token(token) {
                    Some((name, _)) if !lax::is_blank(token) => Some(name),
                    _ => None,
                }
            } else {
                full_token(token).map(|full| full.name)
            };
            match name {
                Some("raw") => self.skip_raw_inside_comment()?,
                Some("comment") => depth += 1,
                Some("endcomment") => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                if !self.tokenizer.is_for_liquid_tag() {
                    self.trim_whitespace = trims_right(token);
                }
                self.line_number = self.tokenizer.line_number();
                return Ok(());
            }
        }
        Err(Self::tag_never_closed("comment"))
    }

    fn skip_raw_inside_comment(&mut self) -> Result<()> {
        while let Some(token) = self.tokenizer.shift() {
            if matches!(full_token_possibly_invalid(token), Some((_, "endraw"))) {
                return Ok(());
            }
        }
        Err(Self::tag_never_closed("raw"))
    }

    pub fn tag_never_closed(block_name: &str) -> Error {
        Error::syntax(format!("'{block_name}' tag was never closed"))
    }

    /// The error for a tag that a block does not accept.
    pub fn unknown_tag_error(tag: &str, block_name: &str, block_delimiter: &str) -> Error {
        if tag == "else" {
            Error::syntax(format!("{block_name} tag does not expect 'else' tag"))
        } else if tag.starts_with("end") {
            Error::syntax(format!(
                "'{tag}' is not a valid delimiter for {block_name} tags. use {block_delimiter}"
            ))
        } else {
            Error::syntax(format!("Unknown tag '{tag}'"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_full_tokens() {
        let full = full_token("{%- if a == b -%}").unwrap();
        assert_eq!(full.name, "if");
        assert_eq!(full.markup, "a == b ");
        let full = full_token("{% # a comment %}").unwrap();
        assert_eq!(full.name, "#");
        let full = full_token("{%\n  render\n 'x' %}").unwrap();
        assert_eq!(full.newlines_before_markup, 2);
        assert!(full_token("{% %}").is_none());
        assert!(full_token("{%- -%}").is_none());
    }

    #[test]
    fn matches_possibly_invalid_tokens() {
        assert_eq!(
            full_token_possibly_invalid("{% endraw %}"),
            Some(("", "endraw"))
        );
        assert_eq!(
            full_token_possibly_invalid("{{ x {% endraw %}"),
            Some(("{{ x ", "endraw"))
        );
        assert_eq!(full_token_possibly_invalid("plain"), None);
    }
}
