//! Minifies the stylesheets and the scripts of a theme, as Shopify does before it serves them.
//!
//! Shopify runs esbuild: whitespace and syntax are minified, names are kept, and stylesheets
//! are rewritten for older browsers. The same is done here with libraries written in Rust, so
//! a file weighs about what it does on Shopify, without being the same to the byte.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use lightningcss::properties::custom::UnparsedProperty;
use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet};
use lightningcss::targets::{Browsers, Features, Targets};
use lightningcss::visit_types;
use lightningcss::visitor::{Visit, VisitTypes, Visitor};
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

/// Whether the library's minifier gets the declarations of this property wrong, so that they
/// are kept out of its reach. esbuild, which Shopify runs, leaves each of them as it is.
fn kept_apart(property: &PropertyId) -> bool {
    use PropertyId::*;
    matches!(
        property,
        // Not parts of `transform`: applied before it, in a fixed order, and cascading on their
        // own. The library writes them into a `transform` beside them, and drops them when
        // `transform` comes after.
        Translate | Rotate | Scale
        // `font` resets what the library does not count as its parts, such as
        // `font-variant-numeric` and `font-feature-settings`: the library moves `font`, or
        // the one it makes of these longhands, after them, which undoes them.
        | Font | FontFamily | FontSize | FontStyle | FontWeight | FontStretch | FontVariantCaps
        | LineHeight
        // The same with `transition`, which resets `transition-behavior`.
        | Transition(_) | TransitionProperty(_) | TransitionDuration(_)
        | TransitionTimingFunction(_) | TransitionDelay(_)
        // `border-image: none` is printed without its value, which a browser ignores.
        | BorderImage(_)
    )
}

/// Turns the declarations the library gets wrong into declarations of tokens: the library keeps
/// these in their place, as they are, without merging them into others or others into them.
/// Their values are minified on their own, then read again as tokens, which `texts` keeps for
/// as long as the stylesheet.
struct KeepApart<'t> {
    texts: &'t Allocator,
}

impl<'i> Visitor<'i> for KeepApart<'i> {
    type Error = std::convert::Infallible;

    fn visit_types(&self) -> VisitTypes {
        visit_types!(PROPERTIES)
    }

    fn visit_property(&mut self, property: &mut Property<'i>) -> Result<(), Self::Error> {
        let property_id = property.property_id();
        if matches!(property, Property::Unparsed(_) | Property::Custom(_))
            || !kept_apart(&property_id)
        {
            return Ok(());
        }
        let printed = property.value_to_css_string(PrinterOptions {
            minify: true,
            ..PrinterOptions::default()
        });
        let Ok(mut value) = printed else {
            return Ok(());
        };
        if value.is_empty() && matches!(property_id, PropertyId::BorderImage(_)) {
            // Each of its parts at its initial value: what `none` says.
            value = "none".to_string();
        }
        let value = self.texts.alloc_str(&value);
        // A custom property is read as tokens, as a declaration the library cannot read is.
        let read =
            Property::parse_string(PropertyId::from("--kept"), value, ParserOptions::default());
        if let Ok(Property::Custom(tokens)) = read {
            *property = Property::Unparsed(UnparsedProperty {
                property_id,
                value: tokens.value,
            });
        }
        Ok(())
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
    // The values of the declarations kept apart, which live as long as the stylesheet.
    let texts = Allocator::default();
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
    let Ok(()) = sheet.visit(&mut KeepApart { texts: &texts });
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
    if parsed.fatal_error || !parsed.diagnostics.is_empty() {
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
        mangle_properties: None,
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

    /// The rules of `source`, minified.
    fn minified_rules(source: &str) -> String {
        stylesheet("/cdn/shop/t/1/assets/a.css", source).unwrap().0
    }

    #[test]
    fn keeps_translate_rotate_and_scale_apart_from_transform() {
        // What issue #47 found: `translate` and `scale` are applied before `transform`, and
        // cascade on their own. Written into `transform`, the first rule turns the other way
        // round and the second no longer resets the `translate` of another stylesheet.
        let source = ".p {\n  transform: rotate(1deg);\n  translate: 10px 0;\n  scale: 1.1;\n}\n\n\
            .b {\n  transform: none;\n  translate: 0 0;\n  scale: 1;\n}\n";
        assert_eq!(
            minified_rules(source),
            ".p{transform:rotate(1deg);translate:10px;scale:1.1}\
             .b{transform:none;translate:0;scale:1}"
        );
        // Written before `transform`, they were dropped.
        assert_eq!(
            minified_rules(".c { translate: 10px 0; transform: rotate(1deg) }"),
            ".c{translate:10px;transform:rotate(1deg)}"
        );
        assert_eq!(
            minified_rules(".m { scale: 1.1; transform: none }"),
            ".m{scale:1.1;transform:none}"
        );
        assert_eq!(
            minified_rules(".n { transform: rotate(1deg); rotate: none }"),
            ".n{transform:rotate(1deg);rotate:none}"
        );
        // In a keyframe and when important too.
        assert_eq!(
            minified_rules(
                "@keyframes rise { from { transform: none; translate: 0 0 } \
                 to { transform: rotate(1deg); translate: 0 50% } }\n\
                 .h { transform: none !important; translate: 0 0 !important }"
            ),
            "@keyframes rise{0%{transform:none;translate:0}\
             to{transform:rotate(1deg);translate:0 50%}}\
             .h{transform:none!important;translate:0!important}"
        );

        // The file of the issue, served through `asset_url`.
        let file = format!(
            "{source}{}",
            ".pad {\n  margin: 0px 0px 0px 0px;\n}\n".repeat(4)
        );
        let served = minify("/cdn/shop/t/1/assets/x.css", &file).unwrap().code;
        assert!(
            served.starts_with(
                ".p{transform:rotate(1deg);translate:10px;scale:1.1}\
                 .b{transform:none;translate:0;scale:1}"
            ),
            "{served}"
        );
    }

    #[test]
    fn keeps_in_place_what_font_and_transition_reset() {
        // `font` resets `font-variant-numeric`, `font-feature-settings` and `font-kerning`:
        // moved after them, or made of longhands written before them, it undoes them.
        assert_eq!(
            minified_rules(
                ".a { font: 12px/1.2 serif; font-feature-settings: \"tnum\"; \
                 font-kerning: none }"
            ),
            ".a{font:12px/1.2 serif;font-feature-settings:\"tnum\";font-kerning:none}"
        );
        let longhands = minified_rules(
            ".b { font-style: normal; font-variant-caps: normal; font-weight: 700; \
             font-stretch: normal; font-size: 12px; line-height: 1; font-family: serif; \
             font-variant-numeric: tabular-nums }",
        );
        assert!(!longhands.contains("font:"), "{longhands}");
        assert!(
            longhands.ends_with("font-family:serif;font-variant-numeric:tabular-nums}"),
            "{longhands}"
        );
        // `transition` resets `transition-behavior`.
        assert_eq!(
            minified_rules(".c { transition: opacity 0.3s; transition-behavior: allow-discrete }"),
            ".c{transition:opacity .3s;transition-behavior:allow-discrete}"
        );
        let longhands = minified_rules(
            ".d { transition-property: display, opacity; transition-duration: 0.3s; \
             transition-timing-function: ease; transition-delay: 0s; \
             transition-behavior: allow-discrete }",
        );
        assert!(!longhands.contains("transition:"), "{longhands}");
        assert!(
            longhands.ends_with(";transition-behavior:allow-discrete}"),
            "{longhands}"
        );
    }

    #[test]
    fn keeps_the_value_of_border_image() {
        // The library printed `border-image:`, which a browser ignores.
        assert_eq!(
            minified_rules(".a { border-width: 1px; border-image: none }"),
            ".a{border-width:1px;border-image:none}"
        );
        assert_eq!(
            minified_rules(".b { border: 1px solid #ff0000; border-image: url(a.png) 30 }"),
            ".b{border:1px solid red;border-image:url(a.png) 30}"
        );
    }

    #[test]
    fn still_merges_what_says_the_same() {
        // Four sides make the shorthand, which sets them and nothing else.
        assert_eq!(
            minified_rules(
                ".a {\n  margin-top: 1px;\n  margin-right: 2px;\n  margin-bottom: 3px;\n  \
                 margin-left: 4px;\n}\n"
            ),
            ".a{margin:1px 2px 3px 4px}"
        );
        // Rules side by side, of the same selector or of the same declarations.
        assert_eq!(
            minified_rules(".l { padding-top: 0 }\n.l { padding-bottom: 0 }"),
            ".l{padding-top:0;padding-bottom:0}"
        );
        assert_eq!(
            minified_rules(".v { color: red }\n.w { color: red }"),
            ".v,.w{color:red}"
        );
        // Not across a rule that could come between.
        assert_eq!(
            minified_rules(".a { color: red }\n.b { color: blue }\n.a { margin: 0 }"),
            ".a{color:red}.b{color:#00f}.a{margin:0}"
        );
        // `transform` is still minified, on its own.
        assert_eq!(
            minified_rules(".t { transform: translate(10px, 0px) rotate(0.5turn) }"),
            ".t{transform:translate(10px)rotate(.5turn)}"
        );
        // So are the values the library keeps apart.
        assert_eq!(
            minified_rules(
                ".f { font-family: \"Helvetica Neue\", sans-serif; font-weight: bold; \
                 transition: opacity 0.30s ease 0s }"
            ),
            ".f{font-family:Helvetica Neue,sans-serif;font-weight:700;transition:opacity .3s}"
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
