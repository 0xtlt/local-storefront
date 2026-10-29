//! Text: paths and positions in messages, JavaScript literals, and the edits a module is written
//! with.

use std::path::Path;

use oxc::diagnostics::OxcDiagnostic;

use crate::javascript::packages;

/// `path` relative to `root`, with `/`; a file of lsf's packages by its path in `node_modules`.
pub(super) fn display(path: &Path, root: &Path) -> String {
    if let Some(relative) = packages::relative(path) {
        return relative;
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// `name:line:column: message` for a diagnostic of oxc about `text`, then the line it is on with
/// a caret under the place:
///
/// ```text
/// src/run.ts:3:17: Unexpected token
///    3 |   const total = (;
///      |                 ^
/// ```
pub(super) fn describe(error: &OxcDiagnostic, name: &str, text: &str) -> String {
    match error.labels.first() {
        Some(label) => {
            let (line, column) = position(text, label.offset() as usize);
            let source = text.lines().nth(line - 1).unwrap_or_default();
            // The caret goes under the character, tabs and wide characters counted as one.
            let pad: String = source
                .chars()
                .take(column.saturating_sub(1))
                .map(|c| if c == '\t' { '\t' } else { ' ' })
                .collect();
            let gutter = " ".repeat(line.to_string().len());
            format!(
                "{name}:{line}:{column}: {}\n {line} | {source}\n {gutter} | {pad}^",
                error.message
            )
        }
        None => format!("{name}: {}", error.message),
    }
}

/// The line and column (from 1) of a byte offset.
pub(super) fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map_or(0, |newline| newline + 1) + 1;
    (line, column)
}

/// A JavaScript string literal.
pub(super) fn quote(text: &str) -> String {
    serde_json::to_string(text).expect("a string serializes")
}

/// `object.name`, or `object["name"]` when the name is not an identifier.
pub(super) fn member(object: &str, name: &str) -> String {
    let identifier = !name.is_empty()
        && name.chars().enumerate().all(|(index, c)| {
            c == '_' || c == '$' || c.is_alphabetic() || (index > 0 && c.is_numeric())
        });
    if identifier {
        format!("{object}.{name}")
    } else {
        format!("{object}[{}]", quote(name))
    }
}

/// `text` with `edits` (start, end, replacement) applied. An edit inside one already applied is
/// skipped (a removed statement takes what it contains with it).
pub(super) fn apply(text: &str, edits: &mut [(u32, u32, String)]) -> String {
    edits.sort_by_key(|(start, end, _)| (*start, std::cmp::Reverse(*end)));
    let mut out = String::with_capacity(text.len() + edits.len() * 16);
    let mut at = 0;
    for (start, end, replacement) in edits.iter() {
        let (start, end) = (*start as usize, *end as usize);
        if start < at {
            continue;
        }
        out.push_str(&text[at..start]);
        out.push_str(replacement);
        at = end;
    }
    out.push_str(&text[at..]);
    out
}
