//! Terminal output.

use slt_core::diagnostics::Diagnostics;

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
        "{} error(s), {} warning(s) in the store data",
        diagnostics.error_count(),
        diagnostics.warning_count()
    );
}
