use std::process::ExitCode;

use lsf_core::store::docs::reference;

#[derive(clap::Args)]
pub struct Args {
    /// Print only the field reference, without the guide.
    #[arg(long)]
    reference: bool,
}

/// The heading the guide ends with; what follows it is replaced by the generated reference.
const REFERENCE_HEADING: &str = "\n## Field reference\n";

/// The guide written into the data directory by `lsf init`: the handwritten explanations
/// followed by the reference of every field, generated from the schema.
pub fn guide() -> String {
    let handwritten = include_str!("../../../../docs/data-format.md");
    let explanations = handwritten
        .split_once(REFERENCE_HEADING)
        .map_or(handwritten, |(explanations, _)| explanations);
    format!(
        "{}{REFERENCE_HEADING}\nEvery type of the format, generated from the JSON Schema the validator uses.\n\n{}",
        absolute_links(explanations),
        reference()
    )
}

/// The guide's links to the other guides (`](checkout.md#scenarios)`) made absolute, to the docs
/// of this version on GitHub: `lsf init` writes the guide into the data directory, where a
/// relative link leads nowhere.
fn absolute_links(text: &str) -> String {
    let base = concat!(
        "https://github.com/0xtlt/local-storefront/blob/v",
        env!("CARGO_PKG_VERSION"),
        "/docs/"
    );
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("](") {
        let (before, after) = rest.split_at(at + 2);
        out.push_str(before);
        let target = after.split([')', '#']).next().unwrap_or_default();
        let relative = target.ends_with(".md")
            && target
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
        if relative {
            out.push_str(base);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

pub fn run(args: Args) -> Result<ExitCode, String> {
    if args.reference {
        print!("{}", reference());
    } else {
        print!("{}", guide());
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_ends_with_the_generated_reference() {
        let guide = guide();
        assert!(guide.starts_with("# Store data format\n"));
        assert_eq!(guide.matches(REFERENCE_HEADING).count(), 1);
        assert!(guide.contains("\n### Product\n"));
        // The link to the separate reference file only makes sense inside the repository.
        assert!(!guide.contains("data-reference.md"));
        // A link to another guide works from the data directory `lsf init` writes it to.
        assert!(!guide.contains("](checkout.md"), "a relative link is left");
        assert!(guide.contains(&format!(
            "](https://github.com/0xtlt/local-storefront/blob/v{}/docs/checkout.md#scenarios)",
            env!("CARGO_PKG_VERSION")
        )));
        assert_eq!(
            absolute_links("[a](https://x.test/b.md) [c](#d) [e](storefront.md)"),
            format!(
                "[a](https://x.test/b.md) [c](#d) [e](https://github.com/0xtlt/local-storefront/blob/v{}/docs/storefront.md)",
                env!("CARGO_PKG_VERSION")
            )
        );
    }
}
