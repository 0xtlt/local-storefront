//! Markup written the way Shopify's strict parser reads it.

use super::meaning::{Meaning, Name, Segment};
use crate::condition::{Condition, Relation};
use crate::expr::Expr;
use crate::variable::Variable;

/// Printed markup, or why the strict parser has no way to say it.
pub type Printed = Result<String, String>;

/// How printed markup follows the markup it replaces.
#[derive(Clone, Debug)]
pub struct Style {
    /// The quote strings are written with.
    pub quote: char,
    /// What goes between the arguments of a tag: `, `, or a comma followed by the line break
    /// and the indentation of the markup it replaces.
    pub separator: String,
    /// Whether the markup separates alternatives with `or` rather than commas (`when`).
    pub uses_or: bool,
}

impl Style {
    /// The style of `markup`. A line of `{% liquid %}` is `single_line`.
    pub fn of(markup: &str, single_line: bool) -> Style {
        let quote = markup
            .chars()
            .find(|c| *c == '\'' || *c == '"')
            .unwrap_or('\'');
        let separator = match markup.find('\n') {
            Some(newline) if !single_line => {
                let indent: String = markup[newline + 1..]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                let line_break = if markup[..newline].ends_with('\r') {
                    "\r\n"
                } else {
                    "\n"
                };
                format!(",{line_break}{indent}")
            }
            _ => ", ".to_string(),
        };
        let uses_or = markup.split_whitespace().any(|word| word == "or");
        Style {
            quote,
            separator,
            uses_or,
        }
    }
}

impl Default for Style {
    fn default() -> Style {
        Style {
            quote: '\'',
            separator: ", ".to_string(),
            uses_or: false,
        }
    }
}

const OPERATORS: [&str; 8] = ["==", "!=", "<>", "<", ">", "<=", ">=", "contains"];

/// A name the strict lexer reads as one identifier: `[a-zA-Z_][\w-]*\??`.
pub fn is_identifier(name: &str) -> bool {
    let body = name.strip_suffix('?').unwrap_or(name);
    let mut chars = body.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A name written as an identifier: a variable, a filter or an argument.
pub fn identifier(name: &str) -> Printed {
    if is_identifier(name) {
        Ok(name.to_string())
    } else {
        Err(format!("`{name}` is not a name the strict parser reads"))
    }
}

/// A string literal.
pub fn string(text: &str, style: &Style) -> Printed {
    let other = if style.quote == '"' { '\'' } else { '"' };
    match [style.quote, other]
        .into_iter()
        .find(|q| !text.contains(*q))
    {
        Some(quote) => Ok(format!("{quote}{text}{quote}")),
        None => Err("a string that holds both quote characters".to_string()),
    }
}

pub fn expr(expr: &Expr, style: &Style) -> Printed {
    meaning(&Meaning::from(expr), style)
}

/// An operand of a condition, where `blank` and `empty` are keywords.
pub fn condition_operand(expr: &Expr, style: &Style) -> Printed {
    meaning(&Meaning::from(expr), style)
}

fn meaning(meaning_of: &Meaning, style: &Style) -> Printed {
    Ok(match meaning_of {
        Meaning::Nil => "nil".to_string(),
        Meaning::Bool(b) => b.to_string(),
        Meaning::Int(i) => i.to_string(),
        Meaning::Float(f) => {
            let text = format!("{f:?}");
            if !text
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')
            {
                return Err(format!("the number {text} has no Liquid notation"));
            }
            text
        }
        Meaning::Str(text) => string(text, style)?,
        Meaning::IntRange(from, to) => format!("({from}..{to})"),
        Meaning::Range(from, to) => {
            format!("({}..{})", meaning(from, style)?, meaning(to, style)?)
        }
        Meaning::Blank => "blank".to_string(),
        Meaning::Empty => "empty".to_string(),
        Meaning::Lookup(name, segments) => {
            let mut out = match name {
                Name::Static(name) if is_variable_name(name, segments.is_empty()) => name.clone(),
                Name::Static(name) => format!("self[{}]", string(name, style)?),
                Name::Dynamic(expr) => format!("self[{}]", meaning(expr, style)?),
            };
            for segment in segments {
                match segment {
                    Segment::Key(key) if is_identifier(key) => {
                        out.push('.');
                        out.push_str(key);
                    }
                    Segment::Key(key) => out.push_str(&format!("[{}]", string(key, style)?)),
                    Segment::Index(index) => out.push_str(&format!("[{}]", meaning(index, style)?)),
                }
            }
            out
        }
        Meaning::Other(value) => return Err(format!("the value {value} has no Liquid notation")),
    })
}

/// Whether a variable can be written by its bare name: an identifier that is not read as a
/// keyword (`contains`) or, on its own, as a literal (`nil`, `true`, `blank`...).
fn is_variable_name(name: &str, alone: bool) -> bool {
    const LITERALS: [&str; 6] = ["nil", "null", "true", "false", "blank", "empty"];
    is_identifier(name) && name != "contains" && !(alone && LITERALS.contains(&name))
}

/// An output's markup: an expression and its filters.
pub fn variable(variable: &Variable, style: &Style) -> Printed {
    let mut out = match &variable.expr {
        Some(value) => expr(value, style)?,
        None if variable.filters.is_empty() => return Ok(String::new()),
        None => return Err("filters without a value".to_string()),
    };
    for filter in &variable.filters {
        out.push_str(" | ");
        out.push_str(&identifier(&filter.name)?);
        let mut args = Vec::new();
        for arg in &filter.args {
            args.push(expr(arg, style)?);
        }
        for (key, value) in &filter.kwargs {
            args.push(format!("{}: {}", identifier(key)?, expr(value, style)?));
        }
        if !args.is_empty() {
            out.push_str(": ");
            out.push_str(&args.join(", "));
        }
    }
    Ok(out)
}

/// The markup of `if`, `elsif` and `unless`.
pub fn condition(condition_of: &Condition, style: &Style) -> Printed {
    let mut out = condition_operand(&condition_of.left, style)?;
    if let Some(operator) = &condition_of.operator {
        if !OPERATORS.contains(&operator.as_str()) {
            return Err(format!(
                "`{operator}` is read as an operator, which renders an error today"
            ));
        }
        let right = match &condition_of.right {
            Some(right) => condition_operand(right, style)?,
            None => "nil".to_string(),
        };
        out.push_str(&format!(" {operator} {right}"));
    }
    if let Some((relation, child)) = &condition_of.child {
        out.push_str(match relation {
            Relation::And => " and ",
            Relation::Or => " or ",
        });
        out.push_str(&condition(child, style)?);
    }
    Ok(out)
}

/// `key: value` arguments, each after the separator of the style.
pub fn arguments(arguments: &[(String, Expr)], style: &Style) -> Printed {
    let mut out = String::new();
    for (key, value) in arguments {
        out.push_str(&style.separator);
        out.push_str(&format!("{}: {}", identifier(key)?, expr(value, style)?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printed(markup: &str) -> String {
        expr(&Expr::parse(markup), &Style::default()).unwrap()
    }

    #[test]
    fn prints_expressions() {
        assert_eq!(printed("a . b ['c'] [0]"), "a.b.c[0]");
        assert_eq!(printed("['a b'].c"), "self['a b'].c");
        assert_eq!(printed("[x].size"), "self[x].size");
        assert_eq!(printed("a['size']"), "a['size']");
        assert_eq!(printed("a.0"), "a['0']");
        assert_eq!(printed("\"it's\""), "\"it's\"");
        assert_eq!(printed("1.2.3"), "1.2");
        assert_eq!(printed("(1..n)"), "(1..n)");
        assert_eq!(printed("(1..3)"), "(1..3)");
        assert_eq!(printed("-"), "self['-']");
        assert_eq!(printed("['nil']"), "self['nil']");
        assert_eq!(printed("nil.a"), "nil.a");
    }

    #[test]
    fn follows_the_style_of_the_markup() {
        assert_eq!(Style::of("'a', b: c", false).separator, ", ");
        assert_eq!(Style::of("'a',\n  b: c", false).separator, ",\n  ");
        assert_eq!(Style::of("'a',\n  b: c", true).separator, ", ");
        assert_eq!(Style::of("'a',\r\n  b: c", false).separator, ",\r\n  ");
        assert_eq!(Style::of("\"a\"", false).quote, '"');
        assert_eq!(string("a'b", &Style::default()).unwrap(), "\"a'b\"");
        assert!(string("a'\"b", &Style::default()).is_err());
    }
}
