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
