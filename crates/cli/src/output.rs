//! Terminal output.

use lsf_core::diagnostics::Diagnostics;

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

/// `1 Function`, `2 Functions`: a number and what it counts, in the singular or the plural.
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_is_singular_for_one_only() {
        assert_eq!(count(0, "Function", "Functions"), "0 Functions");
        assert_eq!(count(1, "Function", "Functions"), "1 Function");
        assert_eq!(count(2, "UI extension", "UI extensions"), "2 UI extensions");
    }
}
