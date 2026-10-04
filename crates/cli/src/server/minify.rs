//! Minifies the stylesheets and the scripts of a theme, as Shopify does before it serves them.
//!
//! Shopify runs esbuild: whitespace and syntax are minified, names are kept, and stylesheets
//! are rewritten for older browsers. The same is done here with libraries written in Rust, so
//! a file weighs about what it does on Shopify, without being the same to the byte.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use lightningcss::stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet};
use lightningcss::targets::{Browsers, Features, Targets};
use oxc::allocator::Allocator;
use oxc::codegen::{Codegen, CodegenOptions, CommentOptions};
use oxc::minifier::{CompressOptions, CompressOptionsKeepNames, Minifier, MinifierOptions};
use oxc::parser::Parser;
use oxc::span::SourceType;
use oxc_compat::EngineTargets;

/// A minified file, and the map that leads a browser's tools back to what was written.
pub struct Minified {
    /// Ends with the link to the source map, as on Shopify.
    pub code: String,
    pub map: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Stylesheet,
    Script,
}

/// What Shopify minifies: the stylesheets and the scripts whose name does not say that they
/// already are.
fn kind(path: &str) -> Option<Kind> {
    let name = path.rsplit('/').next()?.to_ascii_lowercase();
    if name.ends_with(".min.css") || name.ends_with(".min.js") {
        None
    } else if name.ends_with(".css") {
        Some(Kind::Stylesheet)
    } else if name.ends_with(".js") {
        Some(Kind::Script)
    } else {
        None
    }
}

/// Whether the file served at `path` is one that gets minified.
pub fn applies(path: &str) -> bool {
    kind(path).is_some()
}

/// Minifies the file served at `path`. `None` when it is served as written: it is neither a
/// stylesheet nor a script, it cannot be read as one, or nothing would be gained.
pub fn minify(path: &str, source: &str) -> Option<Minified> {
    let kind = kind(path)?;
    // Whatever a theme's file contains, it is served: as written if it cannot be minified.
    let (code, map) = std::panic::catch_unwind(|| match kind {
        Kind::Stylesheet => stylesheet(path, source),
        Kind::Script => script(path, source),
    })
    .ok()??;
    let code = match kind {
        Kind::Stylesheet => format!("{code}\n/*# sourceMappingURL={path}.map */\n"),
        Kind::Script => format!("{code}\n//# sourceMappingURL={path}.map\n"),
    };
    // Shopify serves the file as written when the minified one, link included, is not lighter.
    (code.len() < source.len()).then_some(Minified { code, map })
}

/// What Shopify's minifier rewrites for the browsers it supports, of the level of Safari 14:
/// nesting, the range syntax of media queries, recent color functions and vendor prefixes.
/// The rest is left as written: logical properties and `light-dark()`, which the library
/// would replace with rules of its own.
fn targets() -> Targets {
    let rewritten = Features::Nesting
        | Features::MediaQueries
        | (Features::Colors - Features::LightDark)
        | Features::VendorPrefixes;
    Targets {
        browsers: Some(Browsers {
            safari: Some(14 << 16),
            ..Browsers::default()
        }),
        include: Features::empty(),
        exclude: Features::all() - rewritten,
    }
}

fn stylesheet(path: &str, source: &str) -> Option<(String, String)> {
    let (code, mut map) = match rules(path, source) {
        Ok(printed) => printed,
        Err(unread) => in_parts(path, source, &unread)?,
    };
    // The library names the source relative to a directory: Shopify names it by its path.
    let mut map: serde_json::Value = serde_json::from_str(&map.to_json(None).ok()?).ok()?;
    map["sources"] = serde_json::json!([path]);
    map["sourcesContent"] = serde_json::json!([source]);
    map.as_object_mut()?.remove("sourceRoot");
    Some((code, map.to_string()))
}

/// Minifies the rules of a stylesheet, and maps the result to `source`. When a rule cannot be
/// read, says where each one that cannot is, as an offset in `source`.
fn rules(path: &str, source: &str) -> Result<(String, parcel_sourcemap::SourceMap), Vec<usize>> {
    let targets = targets();
    let options = || ParserOptions {
        filename: path.to_string(),
        ..ParserOptions::default()
    };
    let mut sheet = StyleSheet::parse(source, options()).map_err(|_| {
        // Read again, past what cannot be read, to find every such place.
        let warnings = Arc::new(RwLock::new(Vec::new()));
        let lenient = ParserOptions {
            error_recovery: true,
            warnings: Some(warnings.clone()),
            ..options()
        };
        let _ = StyleSheet::parse(source, lenient);
        let lines: Vec<usize> = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(index, _)| index + 1))
            .collect();
        let warnings = warnings
            .read()
            .map(|found| found.clone())
            .unwrap_or_default();
        warnings
            .iter()
            .filter_map(|warning| warning.loc.as_ref())
            .filter_map(|place| {
                let line = lines.get(place.line as usize)?;
                Some((line + place.column.saturating_sub(1) as usize).min(source.len()))
            })
            .collect::<Vec<usize>>()
    })?;
    sheet
        .minify(MinifyOptions {
            targets,
            ..MinifyOptions::default()
        })
        .map_err(|_| Vec::new())?;
    let mut map = parcel_sourcemap::SourceMap::new("");
    map.add_source(path);
    let printed = sheet
        .to_css(PrinterOptions {
            minify: true,
            source_map: Some(&mut map),
            targets,
            ..PrinterOptions::default()
        })
        .map_err(|_| Vec::new())?;
    Ok((printed.code, map))
}

/// Minifies a stylesheet that has rules the library cannot read: these are kept as written,
/// where they are, and the others are minified around them. Shopify's minifier keeps what it
/// does not know too, and a browser may know better.
fn in_parts(
    path: &str,
    source: &str,
    unread: &[usize],
) -> Option<(String, parcel_sourcemap::SourceMap)> {
    let written = top_level_rules(source);
    let is_unread = |rule: &Range<usize>| unread.iter().any(|offset| rule.contains(offset));
    if unread.is_empty() || !written.iter().any(is_unread) {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    // The line of the result that the next part starts on.
    let mut line = 0;
    let mut map = parcel_sourcemap::SourceMap::new("");
    let mut index = 0;
    while index < written.len() {
        if is_unread(&written[index]) {
            let kept = source[written[index].clone()].trim();
            line += kept.matches('\n').count() as i64 + 1;
            parts.push(kept.to_string());
            index += 1;
            continue;
        }
        // The rules that can be read, up to the next one that cannot.
        let first = index;
        while index < written.len() && !is_unread(&written[index]) {
            index += 1;
        }
        let part = written[first].start..written[index - 1].end;
        // The rest of the file is blanked rather than cut, so that the map names the lines
        // and the columns of the whole file.
        let alone: String = source
            .char_indices()
            .map(|(offset, c)| {
                if part.contains(&offset) || c == '\n' {
                    c
                } else {
                    ' '
                }
            })
            .collect();
        let (code, mut part_map) = rules(path, &alone).ok()?;
        map.add_sourcemap(&mut part_map, line).ok()?;
        line += 1;
        parts.push(code);
    }
    Some((parts.join("\n"), map))
}

/// Where each rule written at the top of a stylesheet starts and ends.
fn top_level_rules(source: &str) -> Vec<Range<usize>> {
    let bytes = source.as_bytes();
    let mut rules = Vec::new();
    let (mut start, mut depth, mut index) = (0, 0usize, 0);
    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = source[index + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| index + 2 + end + 2);
                continue;
            }
            quote @ (b'"' | b'\'') => {
                index += 1;
                while index < bytes.len() && bytes[index] != quote && bytes[index] != b'\n' {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
            }
            b'{' => depth += 1,
            b'}' | b';' => {
                if bytes[index] == b'}' {
                    depth = depth.saturating_sub(1);
                }
                if depth == 0 {
                    rules.push(start..index + 1);
                    start = index + 1;
                }
            }
            _ => {}
        }
        index += 1;
    }
    if !source[start.min(source.len())..].trim().is_empty() {
        rules.push(start..source.len());
    }
    rules
}

fn script(path: &str, source: &str) -> Option<(String, String)> {
    let allocator = Allocator::default();
    // A script is a module when it imports or exports, as browsers and Shopify see it.
    let parsed = Parser::new(&allocator, source, SourceType::unambiguous()).parse();
    if parsed.panicked || !parsed.errors.is_empty() {
        return None;
    }
    let mut program = parsed.program;
    let compress = CompressOptions {
        // No syntax more recent than the one of the file is written in its place.
        target: EngineTargets::from_target("es2015").ok()?,
        keep_names: CompressOptionsKeepNames::all_false(),
        // Nothing is removed for being unused: another script of the page may use it.
        ..CompressOptions::safest()
    };
    let minified = Minifier::new(MinifierOptions {
        mangle: None,
        compress: Some(compress),
    })
    .minify(&allocator, &mut program);
    let printed = Codegen::new()
        .with_options(CodegenOptions {
            minify: true,
            comments: CommentOptions::disabled(),
            source_map_path: Some(PathBuf::from(path)),
            ..CodegenOptions::default()
        })
        .with_source_text(source)
        .with_scoping(minified.scoping)
        .build(&program);
    Some((printed.code, printed.map?.to_json_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "let subscribers = {};\n\n\
        function subscribe(eventName, callback) {\n  \
          if (subscribers[eventName] === undefined) {\n    subscribers[eventName] = [];\n  }\n\n  \
          subscribers[eventName] = [...subscribers[eventName], callback];\n\n  \
          return function unsubscribe() {\n    \
            subscribers[eventName] = subscribers[eventName].filter((cb) => {\n      \
              return cb !== callback;\n    });\n  };\n}\n\n\
        // Tells the subscribers.\n\
        function publish(eventName, data) {\n  \
          if (subscribers[eventName]) {\n    \
            subscribers[eventName].forEach((callback) => {\n      callback(data);\n    });\n  }\n}\n";

    #[test]
    fn minifies_a_script_and_keeps_its_names() {
        let path = "/cdn/shop/t/1/assets/pubsub.js";
        let minified = minify(path, SCRIPT).unwrap();
        // What Shopify serves for this file of Dawn, to the byte.
        assert_eq!(
            minified.code,
            "let subscribers={};function subscribe(eventName,callback){return \
             subscribers[eventName]===void 0&&(subscribers[eventName]=[]),\
             subscribers[eventName]=[...subscribers[eventName],callback],function(){\
             subscribers[eventName]=subscribers[eventName].filter(cb=>cb!==callback)}}\
             function publish(eventName,data){subscribers[eventName]&&\
             subscribers[eventName].forEach(callback=>{callback(data)})}\n\
             //# sourceMappingURL=/cdn/shop/t/1/assets/pubsub.js.map\n"
        );
        let map: serde_json::Value = serde_json::from_str(&minified.map).unwrap();
        assert_eq!(map["version"], 3);
        assert_eq!(map["sources"], serde_json::json!([path]));
        assert_eq!(map["sourcesContent"], serde_json::json!([SCRIPT]));
        assert!(
            map["mappings"]
                .as_str()
                .is_some_and(|mappings| !mappings.is_empty())
        );
    }

    #[test]
    fn leaves_a_script_what_another_one_may_need() {
        let source = "const UNUSED_HERE = 300;\n\n\
            function helper(value) {\n  debugger;\n  const unused = value * 2;\n  \
              if (!window.cache) {\n    window.cache = {};\n  }\n  \
              return value == null ? 'none' : value.name;\n}\n\n\
            class Disclosure extends HTMLElement {\n  \
              connectedCallback() {\n    this.hidden = false;\n  }\n}\n";
        let (code, _) = script("/a.js", source).unwrap();
        // What is declared at the top is there for the other scripts of the page.
        assert!(code.contains("const UNUSED_HERE=300"), "{code}");
        assert!(code.contains("function helper(value)"), "{code}");
        assert!(
            code.contains("class Disclosure extends HTMLElement"),
            "{code}"
        );
        assert!(code.contains("debugger"), "{code}");
        // No syntax more recent than the one of the file.
        assert!(
            !code.contains("?.") && !code.contains("??") && !code.contains("||="),
            "{code}"
        );

        // A module stays one.
        let module = "import { helper } from './helper.js';\n\n\
            export function setup(element) {\n  \
              element.addEventListener('click', () => {\n    helper(element);\n  });\n}\n";
        let (code, _) = script("/module.js", module).unwrap();
        assert!(code.starts_with("import{helper}from"), "{code}");
        assert!(code.contains("export function setup(element)"), "{code}");
    }

    #[test]
    fn minifies_a_stylesheet_and_rewrites_it_for_older_browsers() {
        let source = ".card {\n  /* A card. */\n  color: #ff0000;\n  margin: 0px 0px 0px 0px;\n  \
            user-select: none;\n  padding-inline: 1rem;\n  inset-inline-start: 0;\n\n  \
            &:hover {\n    opacity: 0.5;\n  }\n\n  \
            @media (width >= 750px) {\n    display: grid;\n  }\n}\n";
        let path = "/cdn/shop/t/1/assets/card.css";
        let (code, map) = stylesheet(path, source).unwrap();
        // Whitespace, comments and what a shorter notation says as well.
        assert!(
            code.contains("color:red") && code.contains("margin:0"),
            "{code}"
        );
        // Nesting and ranges are written the way older browsers read them.
        assert!(code.contains(".card:hover{opacity:.5}"), "{code}");
        assert!(
            code.contains("@media (min-width:750px){.card{display:grid}}"),
            "{code}"
        );
        assert!(
            code.contains("-webkit-user-select:none;user-select:none"),
            "{code}"
        );
        assert!(!code.contains('&'), "{code}");
        // Logical properties stay as written.
        assert!(code.contains("padding-inline:1rem"), "{code}");
        assert!(code.contains("inset-inline-start:0"), "{code}");
        assert!(!code.contains(":lang("), "{code}");

        let map: serde_json::Value = serde_json::from_str(&map).unwrap();
        assert_eq!(map["sources"], serde_json::json!([path]));
        assert_eq!(map["sourcesContent"], serde_json::json!([source]));
        assert!(
            map["mappings"]
                .as_str()
                .is_some_and(|mappings| !mappings.is_empty())
        );

        // The file ends with the link to its map, as a script does.
        let longer = source.repeat(3);
        let minified = minify(path, &longer).unwrap();
        assert!(minified.code.len() < longer.len());
        assert!(
            minified
                .code
                .ends_with("}\n/*# sourceMappingURL=/cdn/shop/t/1/assets/card.css.map */\n"),
            "{}",
            minified.code
        );
    }

    #[test]
    fn keeps_as_written_the_rules_it_cannot_read() {
        // The name of a container glued to its condition is not one, which browsers ignore
        // and Shopify's minifier lets through.
        let unread = ".list {\n  display: grid;\n\n  \
            @container list(min-width: 450px) {\n    gap: 1px;\n  }\n}";
        let source = format!(
            ".before {{\n  color: #ff0000;\n}}\n\n{unread}\n\n/* }} */\n\
             .after {{\n  margin: 0px;\n  content: \"}}\";\n}}\n\n.last {{\n  opacity: 0.5;\n}}\n"
        );
        let (code, map) = stylesheet("/a.css", &source).unwrap();
        // The rules around it are minified, and every rule is where it was.
        let lines: Vec<&str> = code.split('\n').collect();
        assert_eq!(lines.first(), Some(&".before{color:red}"));
        assert_eq!(lines[1..lines.len() - 1].join("\n"), unread);
        let after = lines.last().copied().unwrap_or_default();
        assert!(
            after.starts_with(".after{") && after.ends_with(".last{opacity:.5}"),
            "{after}"
        );
        assert!(
            after.contains("margin:0") && after.contains("content:\"}\""),
            "{after}"
        );

        // The map still leads to the lines of the file.
        let map: serde_json::Value = serde_json::from_str(&map).unwrap();
        assert_eq!(map["sources"], serde_json::json!(["/a.css"]));
        assert_eq!(map["sourcesContent"], serde_json::json!([source]));
        let mappings = map["mappings"].as_str().unwrap_or_default();
        assert_eq!(mappings.split(';').count(), lines.len(), "{mappings}");
        assert!(!mappings.split(';').next().unwrap_or_default().is_empty());
        assert!(
            !mappings
                .split(';')
                .next_back()
                .unwrap_or_default()
                .is_empty()
        );
    }

    #[test]
    fn serves_a_file_as_written_when_shopify_does() {
        let script = "function debounce(callback, wait) {\n  let timer;\n  \
            return (...args) => {\n    clearTimeout(timer);\n    \
              timer = setTimeout(() => callback.apply(this, args), wait);\n  };\n}\n";
        let script = script.repeat(3);
        let stylesheet = ".a {\n  color: #ff0000;\n  margin: 0px 0px 0px 0px;\n}\n".repeat(8);
        assert!(minify("/cdn/shop/t/1/assets/global.js", &script).is_some());
        assert!(minify("/cdn/shop/t/1/assets/base.css", &stylesheet).is_some());
        // Its name says that it is minified already.
        assert!(minify("/cdn/shop/t/1/assets/vendor.min.js", &script).is_none());
        assert!(minify("/cdn/shop/t/1/assets/vendor.MIN.css", &stylesheet).is_none());
        // Neither a stylesheet nor a script.
        assert!(minify("/cdn/shop/t/1/assets/data.json", &script).is_none());
        assert!(!applies("/cdn/shop/t/1/assets/icon.svg"));
        assert!(applies("/cdn/shop/t/1/assets/base.css"));
        // Nothing is gained: the link to the map weighs more than what is saved.
        assert!(minify("/cdn/shop/t/1/assets/small.css", "body { margin: 0 }\n").is_none());
        assert!(minify("/cdn/shop/t/1/assets/small.js", "const TIMER = 300;\n").is_none());
        // It cannot be read: the browser gets what the theme has.
        let broken = "function broken( {\n  return 1;\n}\n".repeat(8);
        assert!(minify("/cdn/shop/t/1/assets/broken.js", &broken).is_none());
        let broken = ".a { color: red; } } .b { { color: blue }\n".repeat(8);
        assert!(minify("/cdn/shop/t/1/assets/broken.css", &broken).is_none());
    }
}
