//! The queries of `@media`: cut the way `postcss-media-query-parser` cuts them, and printed
//! by a port of Prettier's printer for them.

use crate::doc::{Doc, concat, group, indent, join, line, text};

use super::misc::{adjust_numbers, adjust_strings, maybe_to_lower_case};

#[derive(Debug, PartialEq, Clone, Copy)]
enum Kind {
    Unknown,
    Keyword,
    MediaType,
    /// `(feature: value)`
    Expression,
}

struct Node<'a> {
    kind: Kind,
    value: &'a str,
}

/// The words of one query, cut at whitespace outside parentheses, each with what it is.
fn parse_media_query(query: &str) -> Vec<Node<'_>> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut start = None;
    let mut level = 0;
    let mut chars = query.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        match start {
            None if c.is_whitespace() => continue,
            None => {
                start = Some(index);
                if c == '(' {
                    level += 1;
                }
            }
            Some(_) => match c {
                '{' | '(' => level += 1,
                ')' | '}' => level -= 1,
                _ => {}
            },
        }
        let ends =
            level == 0 && (c == ')' || chars.peek().is_none_or(|(_, next)| next.is_whitespace()));
        if let (Some(from), true) = (start, ends) {
            let value = &query[from..index + c.len_utf8()];
            let kind = if value.starts_with('(') {
                Kind::Expression
            } else if matches!(value, "not" | "only" | "and") {
                Kind::Keyword
            } else {
                Kind::Unknown
            };
            nodes.push(Node { kind, value });
            start = None;
        }
    }

    // What a word is depends on the words around it.
    for index in 0..nodes.len() {
        if nodes[index].kind != Kind::Unknown {
            continue;
        }
        let kind_at = |nodes: &[Node], at: usize| nodes.get(at).map(|node| node.kind);
        if index > 0 {
            let previous = &nodes[index - 1];
            let kind = if previous.kind == Kind::Expression {
                Kind::Keyword
            } else if matches!(previous.value, "not" | "only") {
                Kind::MediaType
            } else if previous.value == "and" {
                Kind::Expression
            } else if previous.kind == Kind::MediaType {
                match kind_at(&nodes, index + 1) {
                    Some(Kind::Expression) => Kind::Keyword,
                    _ => Kind::Expression,
                }
            } else {
                Kind::Unknown
            };
            nodes[index].kind = kind;
            continue;
        }
        match (kind_at(&nodes, 1), kind_at(&nodes, 2), kind_at(&nodes, 3)) {
            (None, ..) | (Some(Kind::Expression | Kind::Keyword), ..) => {
                nodes[0].kind = Kind::MediaType
            }
            (_, Some(Kind::Expression), _) => {
                nodes[0].kind = Kind::MediaType;
                nodes[1].kind = Kind::Keyword;
            }
            (_, Some(Kind::Keyword), _) => {
                nodes[0].kind = Kind::Keyword;
                nodes[1].kind = Kind::MediaType;
            }
            (_, _, Some(Kind::Expression)) => {
                nodes[0].kind = Kind::Keyword;
                nodes[1].kind = Kind::MediaType;
                nodes[2].kind = Kind::Keyword;
            }
            _ => {}
        }
    }
    nodes
}

/// `(feature: value)`: the feature, and the value when a colon outside strings follows it.
fn print_expression(expression: &str, single_quote: bool) -> String {
    let Some(inner) = expression
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
    else {
        return expression.to_string();
    };
    // The colon that separates the feature from its value, outside `{…}`.
    let mut depth = 0;
    let mut colon = None;
    for (index, c) in inner.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ':' if depth == 0 => {
                colon = Some(index);
                break;
            }
            _ => {}
        }
    }
    let feature = |text: &str| {
        let mut collapsed = String::with_capacity(text.len());
        for c in text.trim().chars() {
            if c != ' ' || !collapsed.ends_with(' ') {
                collapsed.push(c);
            }
        }
        maybe_to_lower_case(&adjust_strings(&collapsed, single_quote))
    };
    match colon {
        Some(colon) => format!(
            "({}: {})",
            feature(&inner[..colon]),
            adjust_numbers(&adjust_strings(inner[colon + 1..].trim(), single_quote))
        ),
        None => format!("({})", feature(inner)),
    }
}

/// The document of the parameters of `@media`.
pub(super) fn print(params: &str, single_quote: bool) -> Doc {
    // The queries, cut at the commas outside parentheses.
    let mut queries = Vec::new();
    let mut start = 0;
    let mut level = 0;
    for (index, c) in params.char_indices() {
        match c {
            '(' => level += 1,
            ')' => level -= 1,
            ',' if level == 0 => {
                queries.push(&params[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    queries.push(&params[start..]);
    let queries: Vec<&str> = queries
        .into_iter()
        .filter(|query| !query.trim().is_empty())
        .collect();
    let last = queries.len().saturating_sub(1);
    let parts = queries
        .iter()
        .enumerate()
        .map(|(index, query)| {
            let words: Vec<Doc> = parse_media_query(query)
                .iter()
                .map(|node| {
                    text(match node.kind {
                        Kind::Expression if node.value.starts_with('(') => {
                            print_expression(node.value, single_quote)
                        }
                        Kind::MediaType => {
                            adjust_numbers(&adjust_strings(node.value, single_quote))
                        }
                        Kind::Keyword => adjust_strings(node.value, single_quote),
                        _ => node.value.to_string(),
                    })
                })
                .collect();
            concat(vec![
                join(&text(" "), words),
                text(if index == last { "" } else { "," }),
            ])
        })
        .collect();
    group(indent(join(&line(), parts)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{PrintOptions, print as print_doc};

    fn fmt(params: &str) -> String {
        print_doc(
            print(params, true),
            &PrintOptions {
                print_width: 80,
                tab_width: 2,
                use_tabs: false,
            },
        )
    }

    #[test]
    fn spaces_features_and_keeps_keywords() {
        assert_eq!(
            fmt("(min-width:100px) AND (max-width:200px),print and (orientation:landscape)"),
            "(min-width: 100px) AND (max-width: 200px), print and (orientation: landscape)"
        );
        assert_eq!(
            fmt("screen  and (MIN-WIDTH : 1.0PX)"),
            "screen and (min-width: 1px)"
        );
        assert_eq!(fmt("(width >= 600px)"), "(width >= 600px)");
        assert_eq!(fmt("print"), "print");
    }
}
