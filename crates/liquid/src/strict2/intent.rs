//! What the markup was meant to do, for the cases where that is clear.
//!
//! A fix ([`super::Issue::fix`]) keeps what renders today, mistakes included. An [`Intent`] does
//! what the markup says it does: a filter in a `render` argument is applied, an `and` in an
//! argument is evaluated, `reversed` reverses. The output changes, which is the point, so an
//! intent is only offered where one reading is obvious, and says what it changes.

use super::print::{self, Style};
use crate::condition::Condition;
use crate::lexer::{MarkupParser, TokenKind};
use crate::variable::Variable;

/// A rewrite that does what the markup meant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Intent {
    /// Tags that run before the tag, without their delimiters
    /// (`assign image_alt = image.alt | default: shop.name`).
    pub prelude: Vec<String>,
    /// The markup that replaces the tag's.
    pub markup: String,
    /// The variables the prelude assigns. Each is renamed when the file already uses its name.
    pub temporaries: Vec<String>,
    /// For a delimiter (`when`): the condition its body goes in, as `{% if condition %}`.
    pub body_condition: Option<String>,
    /// A rewrite of the whole block the tag is in, as replacements in the source: a `case`
    /// that becomes `if`/`elsif`. It replaces the other fields.
    pub block: Vec<(std::ops::Range<usize>, String)>,
    /// What renders differently from today.
    pub change: String,
}

/// Splits markup at the commas that are not inside a string, brackets or parentheses.
pub(crate) fn split_arguments(markup: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (index, c) in markup.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '[' | '(' => depth += 1,
                ']' | ')' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    parts.push(&markup[start..index]);
                    start = index + 1;
                }
                _ => {}
            },
        }
    }
    parts.push(&markup[start..]);
    parts
}

/// `key: value` with a key that is a name.
fn key_value(argument: &str) -> Option<(&str, &str)> {
    let (key, value) = argument.split_once(':')?;
    let key = key.trim();
    print::is_identifier(key).then_some((key, value.trim()))
}

/// A name for a temporary variable made of `parts`: `image_alt`.
fn temporary(parts: &[&str]) -> String {
    let name: String = parts
        .iter()
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.rsplit('/')
                .next()
                .unwrap_or(part)
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("_");
    if name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        name
    } else {
        format!("value_{name}")
    }
}

/// The variable an argument is computed into: `image_alt`, or `price_value` for the `price`
/// argument of `price`.
fn argument_variable(snippet: &str, key: &str) -> String {
    let snippet = snippet.rsplit('/').next().unwrap_or(snippet);
    if snippet == key {
        temporary(&[key, "value"])
    } else {
        temporary(&[snippet, key])
    }
}

/// `render` and `include` whose arguments hold a filter or a condition: each such argument is
/// computed into a variable first.
pub fn partial_call(markup: &str, quoted_name_only: bool, style: &Style) -> Option<Intent> {
    let mut parts = split_arguments(markup).into_iter();
    let head = parts.next()?.trim();
    let call = crate::tags::strict2_partial_call(head, quoted_name_only).ok()?;
    if !call.attributes.is_empty() {
        return None;
    }
    let snippet = call.template_name.as_literal_str().unwrap_or("partial");
    let mut intent = Intent {
        prelude: Vec::new(),
        markup: head.to_string(),
        temporaries: Vec::new(),
        body_condition: None,
        block: Vec::new(),
        change: String::new(),
    };
    let mut changes = Vec::new();
    for part in parts {
        if part.trim().is_empty() {
            continue;
        }
        let (key, value) = key_value(part)?;
        let argument = match Variable::strict2_parse(value) {
            Ok(variable) if variable.filters.is_empty() => value.to_string(),
            Ok(variable) => {
                let name = argument_variable(snippet, key);
                intent.prelude.push(format!(
                    "assign {name} = {}",
                    print::variable(&variable, style).ok()?
                ));
                changes.push(format!("`{key}` gets `{value}`"));
                intent.temporaries.push(name.clone());
                name
            }
            Err(_) => {
                let condition = Condition::strict2_parse(value).ok()?;
                if condition.operator.is_none() && condition.child.is_none() {
                    return None;
                }
                let name = argument_variable(snippet, key);
                intent.prelude.extend([
                    format!("assign {name} = false"),
                    format!("if {}", print::condition(&condition, style).ok()?),
                    format!("assign {name} = true"),
                    "endif".to_string(),
                ]);
                changes.push(format!("`{key}` is true when `{value}` is"));
                intent.temporaries.push(name.clone());
                name
            }
        };
        intent.markup.push_str(&style.separator);
        intent.markup.push_str(&format!("{key}: {argument}"));
    }
    if intent.prelude.is_empty() {
        return None;
    }
    intent.change = changes.join(", ");
    Some(intent)
}

/// A value written in parentheses with its filters: `('X' | downcase)`, which the lax parser
/// reads as the variable `X`.
fn parenthesized(value: &str) -> Option<Variable> {
    let inner = value.trim().strip_prefix('(')?.strip_suffix(')')?;
    let variable = Variable::strict2_parse(inner).ok()?;
    (!variable.filters.is_empty()).then_some(variable)
}

/// An output or `echo` whose value is a filtered value in parentheses.
pub fn output(markup: &str, style: &Style) -> Option<Intent> {
    let variable = parenthesized(markup)?;
    let markup = print::variable(&variable, style).ok()?;
    Some(Intent {
        prelude: Vec::new(),
        change: format!("prints `{markup}`"),
        markup,
        temporaries: Vec::new(),
        body_condition: None,
        block: Vec::new(),
    })
}

/// `assign x = (value | filter)`.
pub fn assign(to: &str, value: &str, style: &Style) -> Option<Intent> {
    let variable = parenthesized(value)?;
    let value = print::variable(&variable, style).ok()?;
    Some(Intent {
        prelude: Vec::new(),
        markup: format!("{} = {value}", print::identifier(to).ok()?),
        temporaries: Vec::new(),
        body_condition: None,
        block: Vec::new(),
        change: format!("`{to}` gets `{value}`"),
    })
}

/// A `for` loop with `reversed` after its attributes, where the lax parser does not see it.
pub fn for_loop(markup: &str) -> Option<Intent> {
    let mut parser = MarkupParser::strict2(markup).ok()?;
    let variable = parser.consume(TokenKind::Id).ok()?;
    if !parser.id("in") {
        return None;
    }
    let collection = parser.expression().ok()?;
    let mut reversed = false;
    let mut attributes = Vec::new();
    loop {
        parser.consume_if(TokenKind::Comma);
        if parser.id("reversed") {
            reversed = true;
        } else if parser.look(TokenKind::Id) && parser.look_ahead(TokenKind::Colon, 1) {
            let key = parser.consume_any();
            if key != "limit" && key != "offset" {
                return None;
            }
            parser.consume(TokenKind::Colon).ok()?;
            attributes.push(format!(" {key}: {}", parser.expression().ok()?));
        } else {
            break;
        }
    }
    parser.consume(TokenKind::EndOfString).ok()?;
    if !reversed {
        return None;
    }
    Some(Intent {
        prelude: Vec::new(),
        markup: format!("{variable} in {collection} reversed{}", attributes.concat()),
        temporaries: Vec::new(),
        body_condition: None,
        block: Vec::new(),
        change: "the loop runs in reverse".to_string(),
    })
}

/// `when 'x' and y`: the strict parser takes no condition after a value, and the lax one ignores
/// it. What it meant is `when 'x'` with its body in `{% if y %}`.
pub fn when_condition(markup: &str, style: &Style) -> Option<Intent> {
    let (value, condition) = split_word(markup, "and")?;
    let mut parser = MarkupParser::strict2(value).ok()?;
    let value = crate::expr::Expr::parse_condition_operand(&parser.expression().ok()?);
    parser.consume(TokenKind::EndOfString).ok()?;
    let condition = Condition::strict2_parse(condition).ok()?;
    let condition = print::condition(&condition, style).ok()?;
    Some(Intent {
        prelude: Vec::new(),
        markup: print::condition_operand(&value, style).ok()?,
        temporaries: Vec::new(),
        change: format!("the body renders only when `{condition}` is true"),
        body_condition: Some(condition),
        block: Vec::new(),
    })
}

/// `markup` split at the first `word` that stands on its own outside strings.
fn split_word<'m>(markup: &'m str, word: &str) -> Option<(&'m str, &'m str)> {
    let mut quote: Option<char> = None;
    for (index, c) in markup.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if markup[index..].starts_with(word) => {
                let before = markup[..index].chars().next_back();
                let after = markup[index + word.len()..].chars().next();
                if before.is_some_and(char::is_whitespace) && after.is_some_and(char::is_whitespace)
                {
                    return Some((markup[..index].trim(), markup[index + word.len()..].trim()));
                }
            }
            None => {}
        }
    }
    None
}

/// A condition written with `=` for `==`.
pub fn condition(markup: &str, style: &Style) -> Option<Intent> {
    let mut fixed = String::with_capacity(markup.len() + 2);
    let mut quote: Option<char> = None;
    let mut previous = ' ';
    let mut chars = markup.chars().peekable();
    let mut replaced = false;
    while let Some(c) = chars.next() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == '='
                && !matches!(previous, '=' | '!' | '<' | '>')
                && chars.peek() != Some(&'=') =>
            {
                fixed.push_str("==");
                previous = c;
                replaced = true;
                continue;
            }
            None => {}
        }
        fixed.push(c);
        previous = c;
    }
    if !replaced {
        return None;
    }
    let condition = Condition::strict2_parse(&fixed).ok()?;
    Some(Intent {
        prelude: Vec::new(),
        markup: print::condition(&condition, style).ok()?,
        temporaries: Vec::new(),
        body_condition: None,
        block: Vec::new(),
        change: "compares with `==`, where the tag renders an error today".to_string(),
    })
}

/// Where the first `|` that is not inside a string is.
pub(crate) fn pipe(markup: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, c) in markup.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == '|' => return Some(index),
            None => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_arguments_at_top_level_commas() {
        assert_eq!(
            split_arguments("'a', b: c | d: 1, 2, e: x['f,g']"),
            vec!["'a'", " b: c | d: 1", " 2", " e: x['f,g']"]
        );
    }

    #[test]
    fn computes_render_arguments_first() {
        let intent = partial_call(
            "'image', alt: image.alt | default: shop.name, size: 2",
            true,
            &Style::default(),
        )
        .unwrap();
        assert_eq!(
            intent.prelude,
            vec!["assign image_alt = image.alt | default: shop.name"]
        );
        assert_eq!(intent.markup, "'image', alt: image_alt, size: 2");
        let intent = partial_call("'price', on_sale: a and b", true, &Style::default()).unwrap();
        assert_eq!(
            intent.prelude,
            vec![
                "assign price_on_sale = false",
                "if a and b",
                "assign price_on_sale = true",
                "endif"
            ]
        );
        assert_eq!(intent.markup, "'price', on_sale: price_on_sale");
        // Nothing to compute, or nothing clear.
        assert_eq!(partial_call("'a', b: c d", true, &Style::default()), None);
        assert_eq!(partial_call("'a', b: c", true, &Style::default()), None);
    }

    #[test]
    fn reads_the_other_mistakes_as_meant() {
        let style = Style::default();
        assert_eq!(
            output("('X' | downcase)", &style).unwrap().markup,
            "'X' | downcase"
        );
        assert_eq!(
            assign("a", "(b | plus: 1)", &style).unwrap().markup,
            "a = b | plus: 1"
        );
        assert_eq!(
            for_loop("i in (1..3) limit: 2 reversed").unwrap().markup,
            "i in (1..3) reversed limit: 2"
        );
        assert_eq!(for_loop("i in (1..3) limit: 2"), None);
        assert_eq!(condition("a = 'b=c'", &style).unwrap().markup, "a == 'b=c'");
        assert_eq!(condition("a == b", &style), None);
    }
}
