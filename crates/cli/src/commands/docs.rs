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
        "{explanations}{REFERENCE_HEADING}\nEvery type of the format, generated from the JSON Schema the validator uses.\n\n{}",
        reference()
    )
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
    }
}
