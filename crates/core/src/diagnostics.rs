//! Problems found in the store data, reported with enough precision for a person or an LLM to
//! fix them without guessing: the file, the JSON path, what is wrong and how to fix it.

use std::fmt;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// A stable identifier for the kind of problem, e.g. `unknown_field`.
    pub code: &'static str,
    /// The file the problem is in, relative to the data directory.
    pub file: String,
    /// The location inside the file as a JSON pointer, e.g. `/variants/0/price`.
    pub path: String,
    pub message: String,
    /// What to change to fix it, when that can be said precisely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Diagnostics {
    pub items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn error(
        &mut self,
        code: &'static str,
        file: &str,
        path: &str,
        message: impl Into<String>,
    ) -> &mut Diagnostic {
        self.push(Severity::Error, code, file, path, message)
    }

    pub fn warning(
        &mut self,
        code: &'static str,
        file: &str,
        path: &str,
        message: impl Into<String>,
    ) -> &mut Diagnostic {
        self.push(Severity::Warning, code, file, path, message)
    }

    fn push(
        &mut self,
        severity: Severity,
        code: &'static str,
        file: &str,
        path: &str,
        message: impl Into<String>,
    ) -> &mut Diagnostic {
        self.items.push(Diagnostic {
            severity,
            code,
            file: file.to_string(),
            path: path.to_string(),
            message: message.into(),
            hint: None,
        });
        self.items.last_mut().expect("just pushed")
    }

    pub fn extend(&mut self, other: Diagnostics) {
        self.items.extend(other.items);
    }

    pub fn has_errors(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.severity == Severity::Error)
    }

    pub fn error_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.severity == Severity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.items.len() - self.error_count()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl Diagnostic {
    pub fn hint(&mut self, hint: impl Into<String>) -> &mut Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{severity}[{}]: {}", self.code, self.message)?;
        let location = if self.path.is_empty() {
            "(root)"
        } else {
            self.path.as_str()
        };
        write!(f, "\n  --> {} at {location}", self.file)?;
        if let Some(hint) = &self.hint {
            write!(f, "\n  hint: {hint}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, item) in self.items.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            writeln!(f, "{item}")?;
        }
        Ok(())
    }
}
