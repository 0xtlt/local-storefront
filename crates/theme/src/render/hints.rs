//! The resource hints Shopify adds to a page on its own: it reads the `<head>` it rendered,
//! and asks the browser to preload the stylesheets and the scripts that block rendering, and
//! to connect to the other origins they come from.

/// The entries of the `Link` header for the `<head>` of a page served from `host`: the origins
/// to connect to, then the files to preload, in the order of the document.
///
/// What `content_for_header` adds is left out: Shopify names its own files after the ones of
/// the theme, which the caller does.
pub fn of_head(html: &str, host: &str) -> Vec<String> {
    let mut preconnects: Vec<String> = Vec::new();
    let mut preloads: Vec<String> = Vec::new();
    let Some(end) = find(html, "</head", 0) else {
        return preloads;
    };
    let head = &html[..end];
    let mut in_header_content = false;
    let mut position = 0;
    while let Some(start) = head[position..].find('<').map(|found| position + found) {
        let rest = &head[start..];
        if rest.starts_with("<!--") {
            position = find(head, "-->", start).map_or(head.len(), |end| end + 3);
            continue;
        }
        let Some(name) = tag_name(rest) else {
            position = start + 1;
            continue;
        };
        let tag_end = end_of_tag(head, start);
        let tag = &head[start..tag_end];
        position = tag_end;
        match name.as_str() {
            // What a browser does not read, or not as markup.
            "noscript" | "style" | "template" | "title" | "textarea" => {
                position = after_closing(head, &name, tag_end);
            }
            "script" => {
                let closing = find(head, "</script", tag_end).unwrap_or(head.len());
                let content = &head[tag_end..closing];
                position = after_closing(head, &name, tag_end);
                if content.contains("shopify.content_for_header.start") {
                    in_header_content = true;
                } else if content.contains("shopify.content_for_header.end") {
                    in_header_content = false;
                }
                let attributes = attributes(tag);
                if in_header_content || !blocking_script(&attributes) {
                    continue;
                }
                if let Some(source) = value(&attributes, "src") {
                    add(
                        &mut preconnects,
                        &mut preloads,
                        source,
                        "script",
                        &attributes,
                        host,
                    );
                }
            }
            "link" => {
                let attributes = attributes(tag);
                if in_header_content || !blocking_stylesheet(&attributes) {
                    continue;
                }
                if let Some(href) = value(&attributes, "href") {
                    add(
                        &mut preconnects,
                        &mut preloads,
                        href,
                        "style",
                        &attributes,
                        host,
                    );
                }
            }
            _ => {}
        }
    }
    preconnects.extend(preloads);
    preconnects
}

type Attributes = Vec<(String, Option<String>)>;

fn add(
    preconnects: &mut Vec<String>,
    preloads: &mut Vec<String>,
    url: &str,
    kind: &str,
    attributes: &Attributes,
    host: &str,
) {
    if let Some(origin) = other_origin(url, host) {
        let preconnect = format!("<{origin}>; rel=\"preconnect\"");
        if !preconnects.contains(&preconnect) {
            preconnects.push(preconnect);
        }
    }
    let mut preload = format!("<{url}>; as=\"{kind}\"; rel=\"preload\"");
    // A file that is asked for without credentials is only reused if it is preloaded so.
    match attributes.iter().find(|(name, _)| name == "crossorigin") {
        Some((_, Some(mode))) if !mode.is_empty() && mode != "anonymous" => {
            preload.push_str(&format!("; crossorigin=\"{mode}\""));
        }
        Some(_) => preload.push_str("; crossorigin"),
        None => {}
    }
    if !preloads.contains(&preload) {
        preloads.push(preload);
    }
}

/// The origin of a URL, when it is not the one of the storefront.
fn other_origin(url: &str, host: &str) -> Option<String> {
    let (scheme, rest) = if let Some(rest) = url.strip_prefix("//") {
        ("https", rest)
    } else if let Some(rest) = url.strip_prefix("https://") {
        ("https", rest)
    } else {
        ("http", url.strip_prefix("http://")?)
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    (!authority.is_empty() && !authority.eq_ignore_ascii_case(host))
        .then(|| format!("{scheme}://{authority}"))
}

/// A stylesheet the browser waits for before it shows the page.
fn blocking_stylesheet(attributes: &Attributes) -> bool {
    let relation = value(attributes, "rel").unwrap_or_default().to_lowercase();
    let mut relations = relation.split_whitespace();
    let media = value(attributes, "media")
        .unwrap_or_default()
        .to_lowercase();
    relations.clone().any(|relation| relation == "stylesheet")
        && !relations.any(|relation| relation == "alternate")
        && !has(attributes, "disabled")
        // A stylesheet for print, as themes write the ones they load late, is not waited for.
        && matches!(media.trim(), "" | "all" | "screen")
}

/// A script the browser runs before it reads the rest of the page.
fn blocking_script(attributes: &Attributes) -> bool {
    let kind = value(attributes, "type").unwrap_or_default().to_lowercase();
    !has(attributes, "async")
        && !has(attributes, "defer")
        && !has(attributes, "nomodule")
        && matches!(
            kind.trim(),
            "" | "text/javascript" | "application/javascript"
        )
}

fn has(attributes: &Attributes, name: &str) -> bool {
    attributes.iter().any(|(known, _)| known == name)
}

fn value<'a>(attributes: &'a Attributes, name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(known, _)| known == name)
        .and_then(|(_, value)| value.as_deref())
        .filter(|value| !value.is_empty())
}

/// Finds `needle`, written in lowercase, whatever the case of the text.
fn find(text: &str, needle: &str, from: usize) -> Option<usize> {
    text.as_bytes()[from..]
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
        .map(|found| from + found)
}

/// The name of the element a `<` opens, in lowercase.
fn tag_name(text: &str) -> Option<String> {
    let name: String = text[1..]
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    let after = text[1 + name.len()..].chars().next();
    (!name.is_empty() && after.is_none_or(|c| c.is_whitespace() || c == '>' || c == '/'))
        .then(|| name.to_ascii_lowercase())
}

/// The index after the `>` that closes the tag opened at `start`: a `>` inside a quoted value
/// does not close it.
fn end_of_tag(text: &str, start: usize) -> usize {
    let mut quote = None;
    for (index, c) in text[start..].char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if open == c => quote = None,
            (None, '>') => return start + index + 1,
            _ => {}
        }
    }
    text.len()
}

/// The index after the closing tag of an element whose content is not markup.
fn after_closing(text: &str, name: &str, from: usize) -> usize {
    find(text, &format!("</{name}"), from).map_or(text.len(), |closing| end_of_tag(text, closing))
}

/// The attributes of a tag, with their names in lowercase and their values decoded.
fn attributes(tag: &str) -> Attributes {
    let mut attributes = Vec::new();
    let inner = tag.trim_start_matches('<').trim_end_matches('>');
    let mut rest = inner
        .trim_start_matches(|c: char| c.is_ascii_alphanumeric())
        .trim_start();
    while !rest.is_empty() {
        let name_end = rest
            .find(|c: char| c.is_whitespace() || c == '=' || c == '/')
            .unwrap_or(rest.len());
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        let mut value = None;
        if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            let (text, remaining) = match after.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let end = after[1..].find(quote).map_or(after.len(), |end| end + 1);
                    (&after[1..end], after.get(end + 1..).unwrap_or_default())
                }
                _ => {
                    let end = after.find(char::is_whitespace).unwrap_or(after.len());
                    (&after[..end], &after[end..])
                }
            };
            value = Some(decode(text));
            rest = remaining;
        }
        if !name.is_empty() {
            attributes.push((name, value));
        }
        rest = rest.trim_start_matches('/').trim_start();
    }
    attributes
}

/// Decodes the entities the filters write in attribute values.
fn decode(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints(head: &str) -> Vec<String> {
        of_head(
            &format!("<html><head>{head}</head><body></body></html>"),
            "shop.test",
        )
    }

    #[test]
    fn preloads_what_blocks_rendering_in_the_order_of_the_document() {
        assert_eq!(
            hints(
                r#"<script src="//shop.test/apps/lock.js" type="text/javascript"></script>
                <link href="//shop.test/cdn/shop/t/1/assets/base.css?v=1&amp;x=2" rel="stylesheet" type="text/css" media="all" />
                <LINK REL="stylesheet" HREF="/local.css">
                <link rel="stylesheet" media="screen" href='theme.css'>"#
            ),
            [
                r#"<//shop.test/apps/lock.js>; as="script"; rel="preload""#,
                r#"<//shop.test/cdn/shop/t/1/assets/base.css?v=1&x=2>; as="style"; rel="preload""#,
                r#"</local.css>; as="style"; rel="preload""#,
                r#"<theme.css>; as="style"; rel="preload""#,
            ]
        );
    }

    #[test]
    fn leaves_out_what_does_not_block_rendering() {
        assert_eq!(
            hints(
                r#"<link rel="stylesheet" href="late.css" media="print" onload="this.media='all'">
                <noscript><link rel="stylesheet" href="late.css"></noscript>
                <link rel="stylesheet" href="off.css" disabled>
                <link rel="alternate stylesheet" href="dark.css">
                <link rel="stylesheet" href="wide.css" media="(min-width: 750px)">
                <link rel="preload" as="font" href="font.woff2" crossorigin>
                <link rel="preconnect" href="https://fonts.example">
                <!-- <link rel="stylesheet" href="commented.css"> -->
                <script src="a.js" defer="defer"></script>
                <script src="b.js" async></script>
                <script src="c.js" type="module"></script>
                <script src="d.js" nomodule></script>
                <script type="application/json" src="e.json"></script>
                <script>document.write('<link rel="stylesheet" href="written.css">')</script>
                <style>a::before { content: '<link rel="stylesheet" href="styled.css">' }</style>
                <template><link rel="stylesheet" href="template.css"></template>"#
            ),
            Vec::<String>::new()
        );
        // Only the head is read.
        assert_eq!(
            of_head(
                r#"<head></head><body><link rel="stylesheet" href="body.css"></body>"#,
                "shop.test"
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            of_head(
                r#"<link rel="stylesheet" href="fragment.css">"#,
                "shop.test"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn connects_to_the_other_origins_first() {
        assert_eq!(
            hints(
                r#"<link rel="stylesheet" href="//shop.test/cdn/base.css">
                <link href="https://fonts.example/css2?family=A&amp;display=swap" rel="stylesheet">
                <script src="//consent.example/banner.js"></script>
                <link rel="stylesheet" href="https://fonts.example/more.css">
                <link rel="stylesheet" href="http://SHOP.test/same.css">"#
            ),
            [
                r#"<https://fonts.example>; rel="preconnect""#,
                r#"<https://consent.example>; rel="preconnect""#,
                r#"<//shop.test/cdn/base.css>; as="style"; rel="preload""#,
                r#"<https://fonts.example/css2?family=A&display=swap>; as="style"; rel="preload""#,
                r#"<//consent.example/banner.js>; as="script"; rel="preload""#,
                r#"<https://fonts.example/more.css>; as="style"; rel="preload""#,
                r#"<http://SHOP.test/same.css>; as="style"; rel="preload""#,
            ]
        );
    }

    #[test]
    fn says_how_a_file_is_asked_for_and_names_it_once() {
        assert_eq!(
            hints(
                r#"<link rel="stylesheet" href="a.css" crossorigin="anonymous">
                <link rel="stylesheet" href="a.css" crossorigin="anonymous">
                <link rel="stylesheet" href="a.css">
                <script src="b.js" crossorigin></script>
                <link rel="stylesheet" href="c.css" crossorigin="use-credentials" title="a > b">"#
            ),
            [
                r#"<a.css>; as="style"; rel="preload"; crossorigin"#,
                r#"<a.css>; as="style"; rel="preload""#,
                r#"<b.js>; as="script"; rel="preload"; crossorigin"#,
                r#"<c.css>; as="style"; rel="preload"; crossorigin="use-credentials""#,
            ]
        );
    }

    #[test]
    fn leaves_what_content_for_header_adds_to_the_caller() {
        assert_eq!(
            hints(
                r#"<link rel="stylesheet" href="before.css">
                <script>window.performance.mark('shopify.content_for_header.start');</script>
                <link rel="stylesheet" media="screen" href="compiled.css">
                <script src="blocking.js"></script>
                <script id="shopify-cfh-end">window.performance.mark('shopify.content_for_header.end');</script>
                <link rel="stylesheet" href="after.css">"#
            ),
            [
                r#"<before.css>; as="style"; rel="preload""#,
                r#"<after.css>; as="style"; rel="preload""#,
            ]
        );
    }
}
