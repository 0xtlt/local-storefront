//! Checks a theme app extension as Shopify does when the app is built and deployed: its
//! folders, its file and content size limits, the schema of each block, and its Liquid, read by
//! Shopify's strict parser.
//!
//! The limits are those of shopify.dev's "Configure theme app extensions": 10 MB of files, 30
//! blocks, 100 locale files of at most 15 KB, 100 KB of Liquid (enforced); 100 KB of CSS and
//! 10 KB of JavaScript, compressed, for the files a schema names (suggested).

use std::path::Path;

use lsf_liquid::strict2::{self, BROKEN_TODAY, IssueKind};
use lsf_liquid::{Environment, Template};
use serde_json::Value as Json;

use crate::theme::schema;

/// The folders an extension may have.
pub const FOLDERS: [&str; 4] = ["assets", "blocks", "locales", "snippets"];

const MAX_TOTAL: u64 = 10 * 1024 * 1024;
const MAX_BLOCKS: usize = 30;
const MAX_LOCALE_FILES: usize = 100;
const MAX_LOCALE_FILE: u64 = 15 * 1024;
const MAX_LIQUID: u64 = 100 * 1024;
const SUGGESTED_CSS: usize = 100 * 1024;
const SUGGESTED_JS: usize = 10 * 1024;
const MAX_NAME: usize = 25;

/// What is wrong with an extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// An error fails the build, as on Shopify; a warning is a limit Shopify suggests.
    pub error: bool,
    /// The file, relative to the extension: `blocks/stars.liquid`. Empty for the extension.
    pub file: String,
    pub line: Option<u32>,
    pub message: String,
}

/// What [`validate`] found, with what it read.
#[derive(Debug, Default)]
pub struct Report {
    pub problems: Vec<Problem>,
    /// The names of the app blocks and of the app embed blocks.
    pub blocks: Vec<String>,
    pub embeds: Vec<String>,
}

impl Report {
    pub fn has_errors(&self) -> bool {
        self.problems.iter().any(|problem| problem.error)
    }

    fn error(&mut self, file: &str, line: Option<u32>, message: String) {
        self.problems.push(Problem {
            error: true,
            file: file.to_string(),
            line,
            message,
        });
    }

    fn warn(&mut self, file: &str, message: String) {
        self.problems.push(Problem {
            error: false,
            file: file.to_string(),
            line: None,
            message,
        });
    }
}

fn kb(bytes: u64) -> String {
    format!("{:.1} KB", bytes as f64 / 1024.0)
}

/// The files of a folder of the extension, sorted, and the folders in it.
fn listing(dir: &Path) -> (Vec<(String, u64)>, Vec<String>) {
    let mut files = Vec::new();
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        match entry.metadata() {
            Ok(meta) if meta.is_dir() => folders.push(name),
            Ok(meta) => files.push((name, meta.len())),
            Err(_) => {}
        }
    }
    files.sort();
    folders.sort();
    (files, folders)
}

/// Checks the extension in `dir`. `compressed` gives the size a file is sent at (gzip), for the
/// suggested limits of the CSS and the JavaScript.
pub fn validate(dir: &Path, env: &Environment, compressed: &dyn Fn(&[u8]) -> usize) -> Report {
    let mut report = Report::default();
    let (root_files, root_folders) = listing(dir);
    let mut total: u64 = root_files.iter().map(|(_, size)| size).sum();
    for folder in &root_folders {
        // What a JavaScript build leaves next to the extension is not deployed.
        if folder == "node_modules" || folder == "dist" {
            continue;
        }
        if !FOLDERS.contains(&folder.as_str()) {
            report.error(
                &format!("{folder}/"),
                None,
                format!(
                    "a theme app extension has only these folders: {} (Shopify rejects `{folder}/`)",
                    FOLDERS.join(", ")
                ),
            );
        }
    }
    let mut liquid: u64 = 0;
    let mut contents: Vec<(String, String)> = Vec::new();
    let mut locale_count = 0;
    for folder in FOLDERS {
        let (files, subfolders) = listing(&dir.join(folder));
        for subfolder in subfolders {
            report.error(
                &format!("{folder}/{subfolder}/"),
                None,
                format!("`{folder}/` cannot have folders: Shopify reads its files only"),
            );
        }
        for (name, size) in files {
            total += size;
            let file = format!("{folder}/{name}");
            match folder {
                "blocks" | "snippets" => {
                    if !name.ends_with(".liquid") {
                        report.error(
                            &file,
                            None,
                            format!("`{folder}/` holds only `.liquid` files"),
                        );
                        continue;
                    }
                    liquid += size;
                    match std::fs::read_to_string(dir.join(&file)) {
                        Ok(source) => contents.push((file, source)),
                        Err(error) => report.error(&file, None, format!("cannot read it: {error}")),
                    }
                }
                "locales" => {
                    if !name.ends_with(".json") {
                        report.error(&file, None, "`locales/` holds only `.json` files".into());
                        continue;
                    }
                    locale_count += 1;
                    if size > MAX_LOCALE_FILE {
                        report.error(
                            &file,
                            None,
                            format!(
                                "{}: a locale file of a theme app extension is at most {}",
                                kb(size),
                                kb(MAX_LOCALE_FILE)
                            ),
                        );
                    }
                    let parsed = std::fs::read_to_string(dir.join(&file))
                        .map_err(|error| error.to_string())
                        .and_then(|source| {
                            serde_json::from_str::<Json>(&source).map_err(|error| error.to_string())
                        });
                    if let Err(error) = parsed {
                        report.error(&file, None, format!("not valid JSON: {error}"));
                    }
                }
                _ => {}
            }
        }
    }
    if total > MAX_TOTAL {
        report.error(
            "",
            None,
            format!(
                "its files take {:.1} MB: Shopify takes at most 10 MB for a theme app extension",
                total as f64 / (1024.0 * 1024.0)
            ),
        );
    }
    if liquid > MAX_LIQUID {
        report.error(
            "",
            None,
            format!(
                "its Liquid takes {}: Shopify takes at most {} of Liquid for a theme app extension",
                kb(liquid),
                kb(MAX_LIQUID)
            ),
        );
    }
    if locale_count > MAX_LOCALE_FILES {
        report.error(
            "locales/",
            None,
            format!(
                "{locale_count} locale files: Shopify takes at most {MAX_LOCALE_FILES} for a theme app extension"
            ),
        );
    }
    let block_count = contents
        .iter()
        .filter(|(file, _)| file.starts_with("blocks/"))
        .count();
    if block_count > MAX_BLOCKS {
        report.error(
            "blocks/",
            None,
            format!(
                "{block_count} blocks: Shopify takes at most {MAX_BLOCKS} for a theme app extension"
            ),
        );
    }
    for (file, source) in &contents {
        check_liquid(&mut report, env, file, source);
        if let Some(name) = file
            .strip_prefix("blocks/")
            .and_then(|name| name.strip_suffix(".liquid"))
        {
            check_schema(&mut report, dir, file, name, source, compressed);
        }
    }
    report
}

/// The Liquid of a file: it parses, and Shopify's strict parser accepts it.
fn check_liquid(report: &mut Report, env: &Environment, file: &str, source: &str) {
    if let Err(error) = Template::parse(env, source) {
        let line = error.line;
        report.error(file, line, error.to_string());
        return;
    }
    for finding in strict2::audit(env, source).findings {
        if matches!(&finding.issue.fix, Err(reason) if reason == BROKEN_TODAY) {
            continue;
        }
        let what = match &finding.tag {
            Some(tag) => format!("{{% {tag} %}}"),
            None => "{{ }}".to_string(),
        };
        match finding.issue.kind {
            IssueKind::Rejected => report.error(
                file,
                Some(finding.line),
                format!(
                    "Shopify's strict Liquid parser rejects this {what}: {} (`lsf check` shows the fix)",
                    finding.issue.message
                ),
            ),
            IssueKind::Reinterpreted => report.problems.push(Problem {
                error: false,
                file: file.to_string(),
                line: Some(finding.line),
                message: format!("Shopify's strict Liquid parser reads this {what} differently"),
            }),
        }
    }
}

/// The `{% schema %}` of a block.
fn check_schema(
    report: &mut Report,
    dir: &Path,
    file: &str,
    name: &str,
    source: &str,
    compressed: &dyn Fn(&[u8]) -> usize,
) {
    let Some(body) = schema::extract_block(source, "schema") else {
        report.error(file, None, "a block needs a {% schema %}".into());
        return;
    };
    let schema: Json = match serde_json::from_str(&lsf_core::json::strip_comments(body)) {
        Ok(Json::Object(schema)) => Json::Object(schema),
        Ok(_) => {
            report.error(file, None, "its {% schema %} is not a JSON object".into());
            return;
        }
        Err(error) => {
            report.error(
                file,
                None,
                format!("its {{% schema %}} is not valid JSON: {error}"),
            );
            return;
        }
    };
    match schema.get("name").and_then(Json::as_str) {
        None => report.error(file, None, "its schema has no `name`".into()),
        Some(text) if !text.starts_with("t:") && text.chars().count() > MAX_NAME => report.warn(
            file,
            format!(
                "its `name` has {} characters: Shopify asks for at most {MAX_NAME}, to fit in the theme editor",
                text.chars().count()
            ),
        ),
        Some(_) => {}
    }
    match schema.get("target").and_then(Json::as_str) {
        Some("section" | "content") => report.blocks.push(name.to_string()),
        Some("head" | "compliance_head" | "body") => report.embeds.push(name.to_string()),
        Some(other) => report.error(
            file,
            None,
            format!(
                "its `target` is `{other}`: Shopify takes `content` (or `section`) for an app block, `head`, `compliance_head` or `body` for an app embed block"
            ),
        ),
        None => report.error(
            file,
            None,
            "its schema has no `target`: `content` for an app block, `head`, `compliance_head` or `body` for an app embed block".into(),
        ),
    }
    if schema.get("enabled_on").is_some() && schema.get("disabled_on").is_some() {
        report.error(
            file,
            None,
            "its schema has both `enabled_on` and `disabled_on`: Shopify takes one of them".into(),
        );
    }
    if let Some(condition) = schema.get("available_if") {
        let reads_app = condition
            .as_str()
            .is_some_and(|text| text.contains("app.metafields"));
        if !reads_app {
            report.error(
                file,
                None,
                "its `available_if` must read an app-data metafield: `{{ app.metafields.<namespace>.<key> }}`".into(),
            );
        }
    }
    let mut ids: Vec<&str> = Vec::new();
    for (index, setting) in schema
        .get("settings")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(kind) = setting.get("type").and_then(Json::as_str) else {
            report.error(file, None, format!("setting {index} has no `type`"));
            continue;
        };
        if matches!(kind, "header" | "paragraph") {
            continue;
        }
        match setting.get("id").and_then(Json::as_str) {
            None => report.error(
                file,
                None,
                format!("setting {index} (`{kind}`) has no `id`"),
            ),
            Some(id) if ids.contains(&id) => {
                report.error(file, None, format!("two settings have the id `{id}`"))
            }
            Some(id) => ids.push(id),
        }
    }
    for (key, suggested, kind) in [
        ("stylesheet", SUGGESTED_CSS, "CSS"),
        ("javascript", SUGGESTED_JS, "JavaScript"),
    ] {
        let Some(asset) = schema.get(key).and_then(Json::as_str) else {
            continue;
        };
        match std::fs::read(dir.join("assets").join(asset)) {
            Ok(bytes) => {
                let size = compressed(&bytes);
                if size > suggested {
                    report.warn(
                        &format!("assets/{asset}"),
                        format!(
                            "{} compressed: Shopify suggests at most {} of {kind} for the `{key}` of a block",
                            kb(size as u64),
                            kb(suggested as u64)
                        ),
                    );
                }
            }
            Err(_) => report.error(
                file,
                None,
                format!("its `{key}` is `{asset}`, and assets/{asset} does not exist"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, content: &str) {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }

    #[test]
    fn reports_what_shopify_rejects() {
        let root = std::env::temp_dir().join(format!("lsf-validate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(
            &root,
            "blocks/stars.liquid",
            "{{ block.settings.title }}{% schema %}{\"name\": \"Stars\", \"target\": \"content\", \"javascript\": \"stars.js\", \"settings\": [{\"type\": \"text\", \"id\": \"title\"}, {\"type\": \"text\", \"id\": \"title\"}]}{% endschema %}",
        );
        write(
            &root,
            "blocks/chat.liquid",
            "{% if a = b %}x{% endif %}{% schema %}{\"name\": \"Chat\", \"target\": \"body\", \"available_if\": \"yes\"}{% endschema %}",
        );
        write(&root, "blocks/notes.txt", "");
        write(&root, "blocks/broken.liquid", "{% if %}");
        write(&root, "src/index.ts", "");
        write(&root, "locales/en.default.json", "{");
        let env = crate::render::environment();
        let report = validate(&root, &env, &|bytes| bytes.len());
        let messages: Vec<String> = report
            .problems
            .iter()
            .map(|problem| format!("{}: {}", problem.file, problem.message))
            .collect();
        let has = |text: &str| messages.iter().any(|message| message.contains(text));
        assert!(
            has("src/: a theme app extension has only these folders"),
            "{messages:?}"
        );
        assert!(has(
            "blocks/notes.txt: `blocks/` holds only `.liquid` files"
        ));
        assert!(has("locales/en.default.json: not valid JSON"));
        assert!(has("blocks/stars.liquid: two settings have the id `title`"));
        assert!(has(
            "blocks/stars.liquid: its `javascript` is `stars.js`, and assets/stars.js does not exist"
        ));
        assert!(has(
            "blocks/chat.liquid: its `available_if` must read an app-data metafield"
        ));
        assert!(has(
            "blocks/chat.liquid: Shopify's strict Liquid parser rejects this {% if %}"
        ));
        assert!(has("blocks/broken.liquid: "));
        assert_eq!(report.blocks, ["stars"]);
        assert_eq!(report.embeds, ["chat"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
