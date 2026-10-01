//! Errors raised while loading a theme or the store data.

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Error {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A JSON file could not be parsed.
    Json { path: String, message: String },
    /// The theme is missing a file or has an invalid structure.
    Theme(String),
    /// The store data is invalid. The diagnostics say where and why.
    Data(crate::diagnostics::Diagnostics),
    /// A Liquid file could not be parsed.
    Liquid(slt_liquid::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Error::Json { path, message } => write!(f, "{path}: {message}"),
            Error::Theme(message) => f.write_str(message),
            Error::Data(diagnostics) => write!(f, "{diagnostics}"),
            Error::Liquid(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<slt_liquid::Error> for Error {
    fn from(error: slt_liquid::Error) -> Self {
        Error::Liquid(error)
    }
}
