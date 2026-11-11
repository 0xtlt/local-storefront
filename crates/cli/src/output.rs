//! Terminal output.

use lsf_core::diagnostics::Diagnostics;
pub use lsf_core::util::count;

/// Prints diagnostics to stderr, followed by a one-line summary.
pub fn print_diagnostics(diagnostics: &Diagnostics) {
    if diagnostics.is_empty() {
        return;
    }
    eprintln!();
    for item in &diagnostics.items {
        eprintln!("{item}\n");
    }
    eprintln!(
        "{}, {} in the store data",
        count(diagnostics.error_count(), "error", "errors"),
        count(diagnostics.warning_count(), "warning", "warnings")
    );
}

/// Set when the error a command stops with is the user's to fix (their extension does not
/// build), not one that kept the command from running: it then exits with `1`, not `2`.
static FIXABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `message`, an error the user fixes in their code: the command exits with `1`.
pub fn fixable(message: String) -> String {
    FIXABLE.store(true, std::sync::atomic::Ordering::Relaxed);
    message
}

/// Whether the error the command stops with is one [`fixable`] made.
pub fn is_fixable() -> bool {
    FIXABLE.load(std::sync::atomic::Ordering::Relaxed)
}

/// A GitHub Actions annotation (`--format github`): a workflow command the runner shows on the
/// line of the file, `::error file=<file>,line=<line>,title=<title>::<message>`.
pub struct Annotation {
    /// `error`, `warning` or `notice`.
    pub level: &'static str,
    /// Relative to the working directory, where the workflow runs.
    pub file: Option<String>,
    pub line: Option<usize>,
    pub title: String,
    /// Several lines are shown as several lines.
    pub message: String,
}

impl Annotation {
    /// The level of an annotation for the severity of a diagnostic.
    pub fn level_of(severity: &str) -> &'static str {
        match severity {
            "error" => "error",
            "warning" => "warning",
            _ => "notice",
        }
    }
}

impl std::fmt::Display for Annotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let data = |text: &str| {
            text.replace('%', "%25")
                .replace('\r', "%0D")
                .replace('\n', "%0A")
        };
        let property = |text: &str| data(text).replace(':', "%3A").replace(',', "%2C");
        let mut properties = Vec::new();
        if let Some(file) = &self.file {
            properties.push(format!("file={}", property(file)));
        }
        if let Some(line) = self.line {
            properties.push(format!("line={line}"));
        }
        if !self.title.is_empty() {
            properties.push(format!("title={}", property(&self.title)));
        }
        write!(
            f,
            "::{} {}::{}",
            self.level,
            properties.join(","),
            data(&self.message)
        )
    }
}

/// A file of the folder `base` as an annotation names it: relative to the working directory,
/// with `/`.
pub fn annotation_path(base: &std::path::Path, file: &str) -> String {
    let path = base.join(file);
    let path = match std::env::current_dir() {
        Ok(cwd) if path.is_absolute() => path
            .strip_prefix(&cwd)
            .map(std::path::Path::to_path_buf)
            .unwrap_or(path),
        _ => path,
    };
    path.components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_annotation_escapes_what_github_reads_as_syntax() {
        let annotation = Annotation {
            level: "error",
            file: Some("theme/sections/a,b.liquid".to_string()),
            line: Some(3),
            title: "lsf check: invalid_schema".to_string(),
            message: "100% wrong\nhint: fix it".to_string(),
        };
        assert_eq!(
            annotation.to_string(),
            "::error file=theme/sections/a%2Cb.liquid,line=3,title=lsf check%3A invalid_schema::100%25 wrong%0Ahint: fix it"
        );
        assert_eq!(
            annotation_path(std::path::Path::new("."), "sections/a.liquid"),
            "sections/a.liquid"
        );
        assert_eq!(
            annotation_path(std::path::Path::new("./theme"), "sections/a.liquid"),
            "theme/sections/a.liquid"
        );
    }
}
