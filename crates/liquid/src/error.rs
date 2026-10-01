//! Errors, formatted the way Liquid prints them into the rendered output.

use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// Raised while parsing a template.
    Syntax,
    /// A filter or tag received an argument it cannot work with.
    Argument,
    ZeroDivision,
    /// A partial (snippet, section, ...) could not be loaded.
    FileSystem,
    /// Nesting or recursion went too deep.
    StackLevel,
    /// A tag was used where it is not allowed (e.g. `include` inside `render`).
    Disabled,
    Standard,
    Internal,
}

#[derive(Clone, Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub line: Option<u32>,
    pub template_name: Option<Arc<str>>,
    /// The offending markup, appended to syntax errors as `in "..."`.
    pub markup_context: Option<String>,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Error {
            kind,
            message: message.into(),
            line: None,
            template_name: None,
            markup_context: None,
        }
    }

    pub fn syntax(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Syntax, message)
    }

    pub fn argument(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Argument, message)
    }

    pub fn standard(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Standard, message)
    }

    pub fn file_system(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::FileSystem, message)
    }

    pub fn zero_division() -> Self {
        Error::new(ErrorKind::ZeroDivision, "divided by 0")
    }

    pub fn stack_level() -> Self {
        Error::new(ErrorKind::StackLevel, "Nesting too deep")
    }

    pub fn internal() -> Self {
        Error::new(ErrorKind::Internal, "internal")
    }

    /// Ruby's arity error, raised when a filter is called with the wrong number of arguments.
    /// `given` and `expected` count the filter input as the first argument, like Ruby does.
    pub fn wrong_arity(given: usize, expected: &str) -> Self {
        Error::argument(format!(
            "wrong number of arguments (given {given}, expected {expected})"
        ))
    }

    pub fn with_line(mut self, line: u32) -> Self {
        if self.line.is_none() {
            self.line = Some(line);
        }
        self
    }

    pub fn with_template(mut self, name: Option<Arc<str>>) -> Self {
        if self.template_name.is_none() {
            self.template_name = name;
        }
        self
    }

    pub fn with_markup_context(mut self, context: impl Into<String>) -> Self {
        if self.markup_context.is_none() {
            self.markup_context = Some(context.into());
        }
        self
    }

    pub fn is_syntax(&self) -> bool {
        self.kind == ErrorKind::Syntax
    }

    /// The message without the `Liquid error (...)` prefix.
    pub fn bare_message(&self) -> String {
        match &self.markup_context {
            Some(context) => format!("{} {}", self.message, context),
            None => self.message.clone(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_syntax() {
            "Liquid syntax error"
        } else {
            "Liquid error"
        })?;
        if let Some(line) = self.line {
            f.write_str(" (")?;
            if let Some(name) = &self.template_name {
                write!(f, "{name} ")?;
            }
            write!(f, "line {line})")?;
        }
        write!(f, ": {}", self.bare_message())
    }
}

impl std::error::Error for Error {}
