//! The tags Shopify's parser accepts only in some files, or once per file: `{% sections %}` in a
//! layout, `{% schema %}` once in a section or a theme block and outside any other tag,
//! `{% javascript %}` and `{% stylesheet %}` once per file, `{% content_for 'blocks' %}` once in
//! a section and never in a snippet, `{% content_for 'block' %}` with a literal `type` and an
//! `id`. Shopify refuses such a file on upload with a Liquid syntax error, and every template
//! that places the section with it.

/// A tag where Shopify's parser refuses it.
#[derive(Debug, PartialEq, Eq)]
pub struct Misplaced {
    pub line: usize,
    pub message: String,
}

/// A tag of a source, outside the bodies of the tags whose body is not Liquid.
struct Tag<'a> {
    name: &'a str,
    /// What follows the name.
    markup: &'a str,
    line: usize,
    /// How many tags it is inside (`if`, `for`, `form`, ...).
    depth: usize,
    /// For a tag whose body is not Liquid (`schema`, `javascript`, ...), its body.
    body: Option<&'a str>,
}

/// Tags whose body is not Liquid: skipped to their end tag.
const RAW_TAGS: [&str; 6] = [
    "schema",
    "raw",
    "javascript",
    "stylesheet",
    "doc",
    "comment",
];

/// Tags with a body that is Liquid, closed by `end<name>`.
const BLOCK_TAGS: [&str; 11] = [
    "if", "unless", "case", "for", "tablerow", "capture", "form", "paginate", "style", "block",
    "partial",
];

/// The tags of a Liquid source, in order, the statements of `{% liquid %}` included.
fn tags(source: &str) -> Vec<Tag<'_>> {
    let line_at = |index: usize| source[..index].matches('\n').count() + 1;
    let mut found = Vec::new();
    let mut open: Vec<&str> = Vec::new();
    let mut position = 0;
    // Updates the open tags with the tag `name`; whether it is an end tag.
    fn nest<'a>(open: &mut Vec<&'a str>, name: &'a str) {
        if let Some(opened) = name.strip_prefix("end") {
            if let Some(index) = open.iter().rposition(|tag| *tag == opened) {
                open.truncate(index);
            }
        } else if BLOCK_TAGS.contains(&name) {
            open.push(name);
        }
    }
    while let Some(start) = [source[position..].find("{{"), source[position..].find("{%")]
        .into_iter()
        .flatten()
        .min()
        .map(|start| position + start)
    {
        if source[start..].starts_with("{{") {
            position = source[start..]
                .find("}}")
                .map_or(source.len(), |end| start + end + 2);
            continue;
        }
        let Some(length) = source[start..].find("%}") else {
            break;
        };
        let markup = source[start + 2..start + length].trim_matches('-').trim();
        position = start + length + 2;
        if markup.starts_with('#') {
            continue;
        }
        let name = markup
            .split(|c: char| c.is_whitespace() || c == '\'' || c == '"')
            .next()
            .unwrap_or_default();
        let rest = markup[name.len()..].trim();
        if RAW_TAGS.contains(&name) {
            // The body runs to the first `{% end<name> %}`.
            let mut end = position;
            let mut body_end = source.len();
            while let Some(found) = source[end..].find("{%") {
                let tag_start = end + found;
                let Some(close) = source[tag_start..].find("%}") else {
                    break;
                };
                let inner = source[tag_start + 2..tag_start + close]
                    .trim_matches('-')
                    .trim();
                end = tag_start + close + 2;
                if inner == format!("end{name}") {
                    body_end = tag_start;
                    break;
                }
            }
            found.push(Tag {
                name,
                markup: rest,
                line: line_at(start),
                depth: open.len(),
                body: Some(&source[position..body_end.max(position)]),
            });
            position = if body_end == source.len() {
                source.len()
            } else {
                end
            };
            continue;
        }
        if name != "liquid" {
            found.push(Tag {
                name,
                markup: rest,
                line: line_at(start),
                depth: open.len(),
                body: None,
            });
            nest(&mut open, name);
            continue;
        }
        // `{% liquid %}`: a tag per line, from the line of `liquid`.
        let first = line_at(start);
        let inside = &source[start + 2..start + length];
        let after = inside
            .find("liquid")
            .map_or("", |index| &inside[index + "liquid".len()..]);
        let mut quiet: Option<String> = None;
        for (offset, statement) in after.lines().enumerate() {
            let statement = statement.trim();
            let name = statement.split_whitespace().next().unwrap_or_default();
            if let Some(end) = &quiet {
                if name == end {
                    quiet = None;
                }
                continue;
            }
            if name == "comment" || name == "raw" || name == "doc" {
                quiet = Some(format!("end{name}"));
                continue;
            }
            if name.is_empty() || statement.starts_with('#') {
                continue;
            }
            found.push(Tag {
                name,
                markup: statement[name.len()..].trim(),
                line: first + offset,
                depth: open.len(),
                body: None,
            });
            nest(&mut open, name);
        }
    }
    found
}

/// The string literal a markup starts with, and what follows it.
fn literal(markup: &str) -> Option<(&str, &str)> {
    let quote = markup.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let end = markup[1..].find(quote)? + 1;
    Some((&markup[1..end], &markup[end + 1..]))
}

/// The `name: value` arguments of a markup, after its first value.
fn arguments(rest: &str) -> Vec<(&str, &str)> {
    let mut parts = Vec::new();
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (index, c) in rest.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == ',' => {
                parts.push(&rest[start..index]);
                start = index + 1;
            }
            None => {}
        }
    }
    parts.push(&rest[start..]);
    parts
        .into_iter()
        .filter_map(|part| part.split_once(':'))
        .map(|(name, value)| (name.trim(), value.trim()))
        .collect()
}

/// What Shopify's parser refuses in a Liquid file of `directory` (`sections`, `blocks`,
/// `snippets`, `layout`, `templates`) for where its tags are.
pub fn problems(directory: &str, source: &str) -> Vec<Misplaced> {
    let mut out = Vec::new();
    let mut refuse = |line: usize, message: String| out.push(Misplaced { line, message });
    let tags = tags(source);
    let section = directory == "sections";
    let block = directory == "blocks";
    let snippet = directory == "snippets";
    let mut schemas = 0;
    let mut javascript = 0;
    let mut stylesheet = 0;
    let mut content_for_blocks = 0;
    for tag in &tags {
        match tag.name {
            "sections" if section => refuse(
                tag.line,
                "Unknown tag 'sections': {% sections %} renders a section group in a layout, not in a section".to_string(),
            ),
            "schema" if snippet => refuse(
                tag.line,
                "Unknown tag 'schema': a snippet has no {% schema %}, only a section or a theme block".to_string(),
            ),
            "schema" if section || block => {
                schemas += 1;
                if schemas == 2 {
                    refuse(
                        tag.line,
                        "Duplicate entries for tag: schema: a file has one {% schema %}".to_string(),
                    );
                }
                if tag.depth > 0 {
                    refuse(
                        tag.line,
                        "'schema' tag must not be nested inside other tags".to_string(),
                    );
                }
                if tag.body.is_some_and(|body| body.trim().is_empty()) {
                    refuse(
                        tag.line,
                        "Invalid tag 'schema': must not be empty".to_string(),
                    );
                }
            }
            "javascript" | "stylesheet" if section || block || snippet => {
                let count = if tag.name == "javascript" {
                    &mut javascript
                } else {
                    &mut stylesheet
                };
                *count += 1;
                if *count == 2 {
                    refuse(
                        tag.line,
                        format!(
                            "Duplicate entries for tag: {0}: a file has one {{% {0} %}}",
                            tag.name
                        ),
                    );
                }
            }
            "content_for" => match literal(tag.markup) {
                Some(("blocks", _)) if snippet => refuse(
                    tag.line,
                    "Unknown tag 'content_for': {% content_for 'blocks' %} renders the blocks of a section or a theme block, not of a snippet".to_string(),
                ),
                Some(("blocks", _)) if section => {
                    content_for_blocks += 1;
                    if content_for_blocks == 2 {
                        refuse(
                            tag.line,
                            "Duplicate entries for 'content_for \"blocks\"': a section renders its blocks once".to_string(),
                        );
                    }
                }
                Some(("blocks", _)) => {}
                Some(("block", rest)) => {
                    let arguments = arguments(rest);
                    let kind = arguments.iter().find(|(name, _)| *name == "type");
                    let id = arguments.iter().any(|(name, _)| *name == "id");
                    let literal_kind =
                        kind.is_some_and(|(_, value)| literal(value).is_some_and(|(_, after)| after.trim().is_empty()));
                    if !id || !literal_kind {
                        refuse(
                            tag.line,
                            format!(
                                "Error in tag 'content_for' - Valid syntax: content_for 'block', type: '<type>', id: '<id>': {}",
                                if kind.is_none() {
                                    "`type` is missing"
                                } else if !literal_kind {
                                    "`type` is not a string literal"
                                } else {
                                    "`id` is missing"
                                }
                            ),
                        );
                    }
                }
                Some((other, _)) => refuse(
                    tag.line,
                    format!(
                        "in 'content_for' - \"{other}\" is not 'blocks' or 'block'"
                    ),
                ),
                None => {}
            },
            _ => {}
        }
    }
    if block && schemas == 0 {
        refuse(
            1,
            "Tag 'schema' is missing: a theme block has a {% schema %}".to_string(),
        );
    }
    out.sort_by_key(|problem| problem.line);
    out
}

#[cfg(test)]
mod tests {
    use super::problems;

    fn found(directory: &str, source: &str) -> Vec<(usize, String)> {
        problems(directory, source)
            .into_iter()
            .map(|problem| {
                let message = problem.message;
                let short = message.split(':').next().unwrap_or_default().to_string();
                (problem.line, short)
            })
            .collect()
    }

    #[test]
    fn the_cases_of_the_issue_are_refused() {
        // #185: a section group in a section, `content_for 'blocks'` twice, a schema in a
        // snippet.
        assert_eq!(
            found(
                "sections",
                "<div>{% sections 'header-group' %}</div>\n{% schema %}\n{ \"name\": \"Group inside\", \"settings\": [] }\n{% endschema %}"
            ),
            [(1, "Unknown tag 'sections'".to_string())]
        );
        assert_eq!(
            found(
                "sections",
                "{% content_for 'blocks' %}{% content_for 'blocks' %}\n{% schema %}\n{ \"name\": \"Blocks twice\", \"blocks\": [ { \"type\": \"@theme\" } ] }\n{% endschema %}"
            ),
            [(
                1,
                "Duplicate entries for 'content_for \"blocks\"'".to_string()
            )]
        );
        assert_eq!(
            found(
                "snippets",
                "<p>snippet</p>\n{% schema %}\n{ \"name\": \"Snippet\" }\n{% endschema %}"
            ),
            [(2, "Unknown tag 'schema'".to_string())]
        );
    }

    #[test]
    fn schema_javascript_and_content_for_rules() {
        let schema = "{% schema %}{\"name\": \"A\"}{% endschema %}";
        // Two schemas; a schema in an `if`; an empty one.
        assert_eq!(
            found("sections", &format!("{schema}\n{schema}")),
            [(2, "Duplicate entries for tag".to_string())]
        );
        assert_eq!(
            found(
                "sections",
                &format!("{{% if true %}}\n{schema}\n{{% endif %}}")
            ),
            [(
                2,
                "'schema' tag must not be nested inside other tags".to_string()
            )]
        );
        assert_eq!(
            found("sections", "{% schema %}  {% endschema %}"),
            [(1, "Invalid tag 'schema'".to_string())]
        );
        // A theme block without a schema; an empty file.
        assert_eq!(
            found("blocks", "<p>no schema</p>"),
            [(1, "Tag 'schema' is missing".to_string())]
        );
        assert_eq!(
            found("blocks", ""),
            [(1, "Tag 'schema' is missing".to_string())]
        );
        // A section without a schema is fine.
        assert!(found("sections", "<p>no schema</p>").is_empty());
        // Two javascript tags; two stylesheet tags in a snippet.
        assert_eq!(
            found(
                "sections",
                &format!(
                    "{{% javascript %}}a(){{% endjavascript %}}\n{{% javascript %}}b(){{% endjavascript %}}\n{schema}"
                )
            ),
            [(2, "Duplicate entries for tag".to_string())]
        );
        assert_eq!(
            found(
                "snippets",
                "{% stylesheet %}a{}{% endstylesheet %}{% stylesheet %}b{}{% endstylesheet %}"
            ),
            [(1, "Duplicate entries for tag".to_string())]
        );
        // `content_for 'blocks'` in a snippet; an unknown target; a static block without its
        // id, without its type, with a variable type.
        assert_eq!(
            found("snippets", "{% liquid\n  content_for 'blocks'\n%}"),
            [(2, "Unknown tag 'content_for'".to_string())]
        );
        assert_eq!(
            found("sections", "{% content_for 'foo' %}"),
            [(
                1,
                "in 'content_for' - \"foo\" is not 'blocks' or 'block'".to_string()
            )]
        );
        for markup in [
            "'block', type: 'text'",
            "'block', id: 't'",
            "'block', type: kind, id: 't'",
        ] {
            assert_eq!(
                found("sections", &format!("{{% content_for {markup} %}}")),
                [(1, "Error in tag 'content_for' - Valid syntax".to_string())],
                "{markup}"
            );
        }
    }

    #[test]
    fn what_shopify_accepts_is_not_reported() {
        let source = "{% comment %}{% schema %}{% endschema %}{% sections 'x' %}{% endcomment %}\
             {% raw %}{% content_for 'foo' %}{% endraw %}\
             {% if a %}{% content_for 'block', type: 'text', id: 'a', color: x %}{% endif %}\
             {% content_for 'blocks' %}\
             {% javascript %}let a = '{% schema %}';{% endjavascript %}{% stylesheet %}a{}{% endstylesheet %}\
             {% schema %}{\"name\": \"A\"}{% endschema %}";
        assert!(found("sections", source).is_empty());
        assert!(found("blocks", source).is_empty());
        // In a layout, `{% sections %}` is where it belongs.
        assert!(
            found(
                "layout",
                "{% sections 'header-group' %}{{ content_for_layout }}"
            )
            .is_empty()
        );
    }
}
