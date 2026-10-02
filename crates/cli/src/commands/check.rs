use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use lsf_core::Theme;
use lsf_core::theme::Revalidate;

/// Tags whose body is not Liquid.
const RAW_TAGS: [&str; 6] = [
    "schema",
    "raw",
    "javascript",
    "stylesheet",
    "doc",
    "comment",
];

/// The filter names after each `|` of one piece of markup, outside string literals.
fn filters_in(markup: &str, line: usize, found: &mut Vec<(String, usize)>) {
    let mut quote: Option<char> = None;
    for (index, c) in markup.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '|' => {
                let name: String = markup[index + 1..]
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    found.push((name, line + markup[..index].matches('\n').count()));
                }
            }
            None => {}
        }
    }
}

/// The filter names used in a Liquid source, with the line each one is on.
fn filters_used(source: &str) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    let mut position = 0;
    while let Some(start) = [source[position..].find("{{"), source[position..].find("{%")]
        .into_iter()
        .flatten()
        .min()
        .map(|start| position + start)
    {
        let output = source[start..].starts_with("{{");
        let Some(length) = source[start..].find(if output { "}}" } else { "%}" }) else {
            break;
        };
        let markup = &source[start + 2..start + length];
        let line = source[..start].matches('\n').count() + 1;
        position = start + length + 2;
        if output {
            filters_in(markup, line, &mut found);
            continue;
        }
        let tag = markup.trim_matches('-').trim();
        let name = tag.split_whitespace().next().unwrap_or_default();
        if tag.starts_with('#') {
            continue;
        }
        if RAW_TAGS.contains(&name) {
            // Skip to the end tag: what is in between is not Liquid.
            position = source[position..]
                .find(&format!("end{name}"))
                .map_or(source.len(), |index| position + index);
            continue;
        }
        if name != "liquid" {
            filters_in(markup, line, &mut found);
            continue;
        }
        // `{% liquid %}` holds one tag per line, comments included.
        let mut comment_depth = 0;
        for (offset, statement) in markup.lines().enumerate() {
            let statement = statement.trim();
            let name = statement.split_whitespace().next().unwrap_or_default();
            match name {
                "comment" => comment_depth += 1,
                "endcomment" if comment_depth > 0 => comment_depth -= 1,
                _ if comment_depth > 0 || statement.starts_with('#') => {}
                _ => filters_in(statement, line + offset, &mut found),
            }
        }
    }
    found
}

pub fn run(theme_dir: &Path) -> Result<ExitCode, String> {
    let env = Arc::new(lsf_core::environment());
    let theme = Theme::open(theme_dir, env.clone(), Revalidate::Never)
        .map_err(|error| error.to_string())?;
    let mut files = 0;
    let mut problems = 0;
    for directory in [
        "layout",
        "templates",
        "templates/customers",
        "sections",
        "blocks",
        "snippets",
    ] {
        for name in theme.files().list(directory).iter() {
            if !name.ends_with(".liquid") {
                continue;
            }
            files += 1;
            let path = format!("{directory}/{name}");
            if let Err(error) = theme.liquid(&path) {
                println!("{path}: {error}");
                problems += 1;
                continue;
            }
            let source = theme.files().read(&path).unwrap_or_default();
            let mut reported = Vec::new();
            for (filter, line) in filters_used(&source) {
                if env.filter(&filter).is_none() && !reported.contains(&filter) {
                    println!(
                        "{path}:{line}: unknown filter `{filter}` (Shopify ignores unknown filters; this one is not implemented here)"
                    );
                    reported.push(filter);
                    problems += 1;
                }
            }
        }
    }
    for directory in ["templates", "sections", "config", "locales"] {
        for name in theme.files().list(directory).iter() {
            if !name.ends_with(".json") {
                continue;
            }
            files += 1;
            let path = format!("{directory}/{name}");
            if let Err(error) = theme.json(&path) {
                println!("{error}");
                problems += 1;
            }
        }
    }
    eprintln!("{files} file(s) checked, {problems} problem(s)");
    Ok(if problems == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::filters_used;

    fn names(source: &str) -> Vec<(String, usize)> {
        filters_used(source)
    }

    #[test]
    fn finds_filters_with_their_line() {
        assert_eq!(
            names("{{ a | upcase }}\n{% assign b = a | plus: 1 | minus: 2 %}"),
            vec![
                ("upcase".to_string(), 1),
                ("plus".to_string(), 2),
                ("minus".to_string(), 2)
            ]
        );
        assert_eq!(
            names("{{ 'a | b' | append: \"|c\" }}"),
            vec![("append".to_string(), 1)]
        );
    }

    #[test]
    fn ignores_what_is_not_liquid() {
        assert!(names("{% comment %} a | b {% endcomment %}").is_empty());
        assert!(names("{%- comment -%} a | b {%- endcomment -%}").is_empty());
        assert!(
            names("{% # a | b %}{% schema %}{\"a\": \"{{ x | y }}\"}{% endschema %}").is_empty()
        );
        assert!(names("{% raw %}{{ a | b }}{% endraw %}").is_empty());
    }

    #[test]
    fn reads_liquid_tags_line_by_line() {
        let source = "{%- liquid\n  comment\n    a || b\n  endcomment\n  # c | d\n  assign e = f | size\n-%}";
        assert_eq!(names(source), vec![("size".to_string(), 6)]);
    }
}
