//! What raw tags and raw elements hold, formatted in its own language.

mod css;
mod js;
mod json;
mod number;

use crate::ast::RawKind;
use crate::doc::Doc;
use crate::options::Options;

/// The document of `source` formatted as `kind`, or `None` when there is no formatter for it
/// or when it does not parse: it is then printed as it was written.
pub(crate) fn format(kind: RawKind, source: &str, options: &Options) -> Option<Doc> {
    match kind {
        RawKind::Css => css::format(source, options),
        RawKind::Javascript => js::format(source, options),
        RawKind::Json => json::format(source, options),
        _ => None,
    }
}
