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
